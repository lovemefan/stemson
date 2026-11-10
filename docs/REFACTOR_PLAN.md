# SanicBoot → Axum 重构方案

> 目标：用 Rust + Axum 重写 SanicBoot，**保留原有功能与设计风格**（Spring Boot 式的
> 约定优于配置、注册表 + 依赖注入、装饰器式校验、统一响应体），同时对原实现中明显不合理
> 的地方提出修正建议（第 5 节）。

---

## 1. 原项目功能清单（必须保留）

| # | 功能 | 原实现位置 | 备注 |
|---|------|-----------|------|
| 1 | YAML 全局配置 + 点号取值 `server.http.port` | `config/Config.py`、`config/config.yaml` | 带默认值、缺省 key 回落 |
| 2 | 配置文件热重载（watchdog 监听） | `config/Config.py` | 文件变更后重新加载 |
| 3 | `@Value` 把配置注入到类属性 | `core/component/value.py` | |
| 4 | `@Autowired` 属性注入（Controller→Service→Repository→Datasource） | `core/component/autowired.py` | 按“小写下划线类名”查注册表 |
| 5 | 自动发现 + 自动注册四类组件 | `core/__init__.py`、`core/datasource/__init__.py` | 扫描配置里的包路径 |
| 6 | 自动注册路由（由模块路径 + 类名推导） | `core/component/controller.py` | 见第 4 节推导结果 |
| 7 | 参数校验装饰器：`NotEmpty/Length/Range/Pattern/EnumString/Assert` | `core/decorator/validateParameters.py` | 支持 json/form/args/files/all |
| 8 | JWT 认证（登录、`/me`、鉴权保护） | `app.py`、`sanic-jwt` | `url_prefix=/v1/api/auth` |
| 9 | 权限校验 `@authority`（按 identity 作用域） | `core/decorator/authority.py` | |
| 10 | 统一响应体 `{message, data, code}` | `model/ResponseBody.py` | |
| 11 | 业务状态码枚举 `SUCCESS="0000" … INTERNAL_SERVER_ERROR="0016"` | `utils/StatusCode.py` | |
| 12 | 全局异常处理 → 统一响应体 | `app.py` | 404/超时/未授权/业务异常 |
| 13 | CORS 中间件（含 OPTIONS 预检） | `app.py` | |
| 14 | MySQL 数据源：连接池、事务、自动 commit/rollback、批量、多语句 | `core/datasource/MysqlDataBasePool.py` | |
| 15 | Redis 连接池单例（同步/异步两种用法） | `core/datasource/RedisConnectPool.py` | |
| 16 | 雪花算法 ID 生成器（时钟回拨保护） | `utils/snowflake.py` | |
| 17 | MD5 密码工具（带固定盐） | `utils/md5Utils.py` | |
| 18 | 日志：彩色控制台 + 轮转文件 | `utils/logger.py` | |
| 19 | Swagger/OpenAPI 文档 | `sanic-openapi` | `/swagger` |
| 20 | 用户业务：登录、增删改查、用户组 | `controller/service/repository`、`mysql/create_database.sql` | |

## 2. 目标目录结构（一一对应）

```
Cargo.toml                     # 根 crate：sanicboot（lib + bin）
sanicboot-macros/              # 过程宏 crate（对应“装饰器”）
src/
  main.rs                      # 原 backend/app.py：启动、中间件、全局错误处理
  lib.rs                       # 模块导出
  config/
    mod.rs                     # 原 Config.py：点号取值 + 热重载 + BaseConfig
    config.yaml                # 原 config.yaml（结构保持一致）
  core/
    registry.rs                # 原 core/__init__.py：四张注册表
    component/
      controller.rs            # 原 @Controller + 自动路由
      service.rs               # 原 @Service（单例）
      repository.rs            # 原 @Repository
      autowired.rs             # 原 @Autowired（构造期解析注册表）
      value.rs                 # 原 @Value
    datasource/
      mod.rs                   # 数据源注册 @Datasource("mysql") + trait
      mysql.rs                 # 原 MysqlDataBasePool.py
      redis.rs                 # 原 RedisConnectPool.py
    decorator/
      validate.rs              # 原 validateParameters.py 的六个校验器
      authority.rs             # 原 authority.py
  model/
    response.rs                # 原 ResponseBody.py → ApiResponse<T>
    user.rs                    # 原 User.py
    user_identity.rs           # 原 UserIdentity.py
    controller.rs / service.rs / repository.rs   # 原三个 Base 类 → trait
  controller/user/user_controller.rs   # 原 UserController.py
  service/user_service.rs               # 原 UserService.py
  repository/user_repository.rs         # 原 UserRepository.py
  exception/mod.rs                      # 原 exception/*.py
  utils/
    logger.rs  snowflake.rs  md5.rs  status_code.rs  text.rs
config/config.yaml             # 运行时配置（与 Python 版同结构）
mysql/create_database.sql      # 原样保留
tests/                         # 集成测试
```

> Python 原实现移到 `legacy/python/` 保留备查（不参与构建）。

## 3. 技术选型

| 能力 | 选择 |
|------|------|
| Web | `axum` 0.8 + `tokio` |
| 中间件 | `tower` / `tower-http`（CORS、Trace、RequestId） |
| 配置 | `serde_yaml_ng` + 自研点号取值（保持 `Config.get("a.b.c", default)` 语义） |
| 热重载 | `notify` + `arc-swap`（不阻塞读） |
| DI | `inventory`（链接期自动注册）+ 过程宏（`#[service]` `#[repository]` `#[controller]` `#[derive(Autowired)]`） |
| 数据库 | `sqlx`（MySQL，**运行时 SQL**，编译不依赖数据库） |
| 缓存 | `redis`（`tokio` + `connection-manager`） |
| JWT | `jsonwebtoken` |
| OpenAPI | `utoipa` + `utoipa-swagger-ui` |
| 日志 | `tracing` + `tracing-subscriber` + `tracing-appender`（滚动文件） |
| 密码 | `md5`（兼容旧数据）+ 可选 `argon2` |
| 其它 | `chrono`、`regex`、`thiserror`、`once_cell`、`md-5` |

## 4. 原项目实际行为（重要）

### 4.1 自动路由的真实结果

`@Controller` 的路径 = `(类所在模块去掉配置前缀) + 类名的 snake_case`：

| 类 | 实际路由 |
|----|---------|
| `ValidatePassword` | `POST /user/user_controller/validate_password` |
| `AddUser` | `POST /user/user_controller/add_user` |
| `GetUserInfo` | `POST /user/user_controller/get_user_info` |
| `DeleteUser` | `POST /user/user_controller/delete_user` |
| `ModifyUser` | `POST /user/user_controller/modify_user` |
| `GetAllInfo`（显式 `@Controller("/getall")`） | `POST /getall` |
| `Authenticate` / `RetrieveUser`（未加 `@Controller`） | 不是路由，供 JWT 回调使用 |
| sanic-jwt | `POST /v1/api/auth`、`/v1/api/auth/me`、`/verify`、`/refresh` |

> 注意：仓库里的 `user_api.md` 写的是 `/v1/api/user/add_user`，那是 **v1.0 Blueprint 时代**
> 的旧地址，与当前 v2 代码不一致。重构时需要明确以哪个为准（见第 6 节 Q2）。

### 4.2 其它实际行为

- 所有业务接口都是 `POST`（即便是查询）。
- 校验失败返回 HTTP `401`；未授权返回 `403`；用户不存在返回 `401`。
- `ResponseBody` 的 `code` 字段：一部分用枚举 `.value`（如 `"0010"`），一部分用 `.name`
  （如 `"ADD_USER_SUCCESS"`），两种混用。
- JWT：`MyJWTConfig.user_id = "username"`，但 `RetrieveUser.retrieve_user` 读
  `payload["user_id"]`，二者不一致 → 原代码 `/me` 与 `inject_user` 实际上取不到用户
  （这属于 bug，重构时修正为统一的 `user_id` claim）。

## 5. 建议修正的不合理之处

1. **SQL 注入「黑名单」既无效又误杀**（最重要）
   原实现用 f-string 拼 SQL，再靠关键词黑名单兜底：既挡不住注入（`/**/`、编码变形、
   数字型注入都绕过），又会把合法昵称 `Tom or Jerry`、`+86`、`100%` 直接判为攻击。
   → 改为 **参数化查询**（`?` 占位 + bind），彻底删除黑名单；保留“Repository 里声明
   SQL、数据源自动执行/提交/回滚/批量”的写法。

2. **密码存储**：MD5 + 硬编码盐（`人社厅语音项目`）强度过低，且盐写死在代码里。
   → 建议：登录时兼容旧 MD5 校验，新写入用 Argon2id；成功后按需重算迁移。
   若必须零改动，则保留 MD5（需接受同样的弱安全性）。

3. **HTTP 状态码语义错误**：缺参数返回 401（应 400）、未授权返回 403（应 401）、
   用户不存在返回 401（应 404）。→ 建议返回语义正确的状态码，业务码放在响应体里。

4. **统一响应体的 `code` 混用 name/value** → 统一为 4 位业务码字符串，另可选加
   `status` 字段承载枚举名。

5. **`@Autowired` 要求被注入方法不能有 `self`**（Python 类装饰器的限制，文档里专门
   吐槽过）→ Rust 改为构造函数注入，方法有 `&self`，问题自然消失。

6. **`UserService.add_user` 里 `self.get_user_id(user).uid` 少了 `await`**（必然报错），
   且 `login()` 中 `where username='{user.uid}'` 把 uid 拼进了 username 条件 → 修复。

7. **配置热重载只对“读值”有效**，数据库/端口等改动不会生效 → 明确文档：仅动态配置
   热生效，基础设施配置需重启。

8. **CORS `allow_origin = "*"` 且默认允许 Authorization 头**，生产环境应可配置白名单。

9. **全局兜底异常把内部错误信息直接返回给客户端**（`exception.args[0]`）→ 建议对外
   返回通用消息 + 内部日志记录详情。

## 6. 已确认的决策

| 编号 | 问题 | 结论 |
|------|------|------|
| Q1 | 依赖注入实现方式 | 完整还原：`inventory` 链接期自动注册 + 过程宏，新增组件不改中心注册代码 |
| Q2 | 路由规则 | 保留自动推导（`/user/user_controller/add_user`），支持 `#[controller("/xxx")]` 与 `#[get("/sub")]` 覆盖 |
| Q3 | 响应体与状态码 | `code` 统一为 4 位业务码；HTTP 状态码语义正确（400/401/403/404/409） |
| Q4 | 数据访问 | 参数化查询，保留「Repository 声明 SQL、数据源自动执行」的写法 |
| Q5 | 密码 | 全部改为 Argon2id（需重置旧密码） |
| Q6 | 目录与交付 | Rust 在根目录，Python 移到 `legacy/python/`；交付工程 + 测试 + 文档 + Docker |

## 7. 实施结果

### 7.1 功能完成度（对应第 1 节）

| # | 功能 | 状态 | 实现位置 |
|---|------|------|----------|
| 1 | YAML 点号配置 | 完成 | `src/config.rs` |
| 2 | 配置热重载 | 完成（实测） | `Config::watch` + `arc-swap` |
| 3 | `@Value` | 完成 | `#[value("...")]` 字段 + `core/component/value.rs` |
| 4 | `@Autowired` | 完成 | `#[derive(Autowired)]` 字段注入 |
| 5 | 自动发现/注册 | 完成 | `inventory` + 四个宏 |
| 6 | 自动路由 | 完成（路径与原版逐条一致） | `core/route.rs` + `#[controller]` |
| 7 | 6 个校验器 | 完成 | `core/decorator/validate.rs` + `#[validate(...)]` |
| 8 | JWT 认证 | 完成（`jsonwebtoken`） | `src/auth.rs`、`controller/auth_controller.rs` |
| 9 | `@authority` | 完成 | `#[authority([...])]` |
| 10 | 统一响应体 | 完成 | `model/response.rs` |
| 11 | 业务码 | 完成（16 个 + 新增 0017） | `model/status_code.rs` |
| 12 | 全局异常处理 | 完成（含 404/405/timeout/panic） | `src/exception.rs`、`src/app.rs` |
| 13 | CORS | 完成（可配置白名单） | `src/app.rs` |
| 14 | MySQL 数据源 | 完成（参数化、事务、批量） | `datasource/mysql.rs` |
| 15 | Redis | 完成（懒连接，基础命令） | `datasource/redis.rs` |
| 16 | 雪花 ID | 完成（位布局一致） | `utils/snowflake.rs` |
| 17 | 密码工具 | 改为 Argon2id | `utils/hash.rs` |
| 18 | 日志 | 完成（颜色 + 文件轮转 + 模板解析） | `utils/logger.rs` |
| 19 | Swagger | 完成（`utoipa`，静态资源内置） | `src/openapi.rs` |
| 20 | 用户业务 | 完成（登录/增删改查/用户组） | `controller/`、`service/`、`repository/` |

### 7.2 第 5 节建议的落地

1. SQL 注入：已改为参数化查询，黑名单删除 ✅
2. 密码：Argon2id（旧 MD5 不兼容，需重置） ✅
3. HTTP 状态码：已修正 ✅
4. `code` 混用：已统一 ✅
5. `@Autowired` 的 `self` 限制：Rust 构造函数注入后消失 ✅
6. 功能性 bug：`await`、`login` 的 uid 条件、`ValidatePassword` 未定义变量等已修 ✅
7. 热重载边界：文档明确「仅取值型配置热生效」 ✅
8. CORS：支持显式白名单 ✅
9. 内部错误外泄：对外只返回通用消息，详情写日志 ✅

### 7.3 验证

- `cargo test`：75 个测试（60 单元 + 15 集成），不依赖数据库；
- `cargo clippy --all-targets -- -D warnings`：零告警；
- `scripts/e2e-test.sh`：对真实 MySQL 8 执行 23 项端到端检查，全部通过
  （登录、鉴权、校验、增删改查、重复用户名、越权、删除后查询）；
- 手工验证：配置热重载、Swagger UI、OpenAPI 文档、release 构建。
