# 从 Python 版迁移

本文记录重构中**有意为之**的行为差异、需要人工处理的步骤和已知限制。
逐项功能是否保留见 [REFACTOR_PLAN.md](REFACTOR_PLAN.md)。

## 1. 设计决策

| 决策 | 选择 | 影响 |
|------|------|------|
| 依赖注入 | 完整还原：`inventory` 链接期注册 + 过程宏 | 新增组件无需改中心注册代码；宏代码较多 |
| 路由 | 保留 Python 的自动推导规则，支持显式路径覆盖 | 现网调用方 URL 不变 |
| 响应体 | `code` 统一为 4 位业务码，HTTP 状态码语义正确 | 客户端若依赖「校验失败=401」需调整 |
| 数据访问 | 参数化查询，保留 Repository 里声明 SQL 的风格 | 黑名单删除，合法数据不再误杀 |
| 密码 | 全部改为 Argon2id | 旧 MD5 密码失效，需要重置 |
| 目录 | Rust 在根目录，Python 移到 `legacy/python/` | 新老代码对照方便 |

## 2. 保持不变

- 四层组件与注入链：Controller → Service → Repository → Datasource；
- 组件名规则：类名 `CamelCase` → `snake_case`（`UserService` → `user_service`）；
- 自动路由规则与结果（见下表）；
- `ResponseBody` 的三个字段名：`message`、`data`、`code`；
- 16 个业务码的数值（`SUCCESS="0000"` … `INTERNAL_SERVER_ERROR="0016"`）；
- 6 个参数校验器的语义、作用域与「缺参数/非法值」区分；
- `User.to_dict()` 的字段与时间格式 `%Y-%m-%d %H:%M:%S`；
- 雪花算法参数（epoch `1288834974657`、worker `2`、datacenter `1`）；
- MySQL 表结构（除 `password` 列长度与种子数据）；
- 日志配置项名称与 Python 风格格式串；
- CORS 默认允许 `*`；
- 用户业务语义（管理员才能增删改查，普通用户被拒绝）。

### 路由对照（实测行为，不是旧文档）

| 操作 | Python 实际路由 | Rust 路由 |
|------|-----------------|-----------|
| 校验密码 | `POST /user/user_controller/validate_password` | 同 |
| 新增用户 | `POST /user/user_controller/add_user` | 同 |
| 查询用户 | `POST /user/user_controller/get_user_info` | 同 |
| 删除用户 | `POST /user/user_controller/delete_user` | 同 |
| 修改用户 | `POST /user/user_controller/modify_user` | 同 |
| 列出用户 | `POST /getall` | 同 |
| 登录 | `POST /v1/api/auth` | 同 |
| 当前用户 | `GET /v1/api/auth/me` | 同 |
| 校验 token | `GET /v1/api/auth/verify` | 同 |
| 续签 | `POST /v1/api/auth/refresh` | 同 |

仓库里的 `user_api.md` 写的是 v1.0 Blueprint 时代的 `/v1/api/user/...`，与当前 Python
代码本身就不一致。本次以**实际代码**为准；若你确实需要旧文档的 URL，可给控制器加
显式路径，或再加一层别名路由。

## 3. 行为变化（含理由）

### 3.1 SQL 注入防护

- **原来**：f-string 拼 SQL，再用关键词黑名单兜底。
  既挡不住注入（`/**/`、数字型注入、编码变形），又会拒绝 `Tom or Jerry`、`100%`、
  `+86`、包含 `select` 的昵称等合法数据。
- **现在**：所有查询使用 `?` 占位 + 绑定参数；黑名单与 `filter_invalid_character` 删除。
- **迁移影响**：如果业务里有依赖「黑名单报错」的行为，需要显式改写。

### 3.2 密码

- **原来**：`md5("人社厅语音项目" + password)`，客户端负责 MD5；列 `char(32)`。
- **现在**：服务端 Argon2id（随机盐），API 直接收明文（务必走 TLS），
  列改为 `varchar(255)`。
- **迁移影响**：
  1. 执行新的 `mysql/create_database.sql`（或手动 `ALTER TABLE user MODIFY password varchar(255) NOT NULL`）；
  2. 旧用户的 MD5 密码**无法**通过校验，需要重置；
  3. 客户端不再需要先做 MD5。

### 3.3 HTTP 状态码

| 场景 | 原来 | 现在 |
|------|------|------|
| 缺参数 / 参数非法 | 401 | 400 |
| 未认证 | 403 / 401 混用 | 401 |
| 身份不足 | 403 | 403 |
| 用户不存在 | 401 | 404 |
| 用户名已存在 | 未定义（500） | 409（`0017`） |
| 路由不存在 | 404 | 404（统一响应体） |
| 方法不允许 | — | 405（统一响应体） |

业务码仍是唯一可靠的判定依据；只依赖 HTTP 状态的客户端需要适配。

### 3.4 响应体 `code`

- **原来**：一部分用枚举值（`"0010"`），一部分用枚举名（`"ADD_USER_SUCCESS"`）。
- **现在**：统一为 4 位数值字符串；枚举名保留在代码与文档里（`StatusCode::name()`）。
- **迁移影响**：把 `code == "ADD_USER_SUCCESS"` 之类的判断改为 `"0000"`。

### 3.5 列表数据的位置

- **原来**：`get_all_user_information` 把用户列表塞进 `message`。
- **现在**：列表放在 `data`，`message` 固定为可读文本。
- **迁移影响**：读取 `message` 当数组的客户端改为读 `data`。

### 3.6 JWT claim

- **原来**：`MyJWTConfig.user_id = "username"`，但 `retrieve_user` 读
  `payload["user_id"]`；两者不一致，导致 `/me`、`@protected` 实际拿不到用户。
- **现在**：只有一个 `user_id`（雪花 id），另有 `username`、`role`、`identity`、`scopes`。
- **迁移影响**：旧 token 全部失效（claim 结构不同），重新登录即可。

### 3.7 新用户身份

- **原来**：`add_user` 不写 `identity`，依赖列默认值 `1`（超级管理员）——
  任何管理员创建的用户都变成超管。
- **现在**：`role=1` → `identity=2`；`role=0` → `identity=3`；列默认值改为 `3`。

### 3.8 `getall` 的鉴权

- **原来**：`GetAllInfo` 没有任何鉴权装饰器（任何人可拉全量用户），
  并且要求一个无意义的 `test` 字段。
- **现在**：`#[protected]` + `#[authority([UserIdentity::Admin])]`；去掉 `test` 字段。

### 3.9 邮箱校验

- **原来**：`^([a-z0-9A-Z]+[-|\.]?)+[a-z0-9A-Z]@(...)$`，会拒绝
  `first_last@example.com`、`user+tag@example.com`，还允许 `|`。
- **现在**：`^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$`。

### 3.10 已修复的功能性 bug

| 位置 | 原问题 | 处理 |
|------|--------|------|
| `UserService.add_user` | `self.get_user_id(user).uid` 少了 `await` | 事务内先取 uid 再插入 |
| `UserRepository.login` | `where username='{user.uid}'` 把 uid 塞进 username | `WHERE uid = ?` |
| `ValidatePassword` | 失败分支引用未定义变量 | 返回 `0006` |
| `ModifyUser` | 只改非空字段时 `status`/`role` 可能为 None 崩溃 | 未提供字段保持原值 |
| `add_user_into_group` | 缺少事务 | 与插入用户同事务 |

## 4. 移除的功能

| 移除项 | 原因 | 替代 |
|--------|------|------|
| `server.component.*` 扫包配置 | Rust 没有运行时模块扫描；注册在链接期完成 | 宏 + `inventory`，无需配置 |
| SQL 关键词黑名单 `filter_invalid_character` | 无效且误杀 | 参数化查询 |
| MD5 工具（`utils/md5Utils.py`） | 不安全 | `utils/hash.rs`（Argon2id） |
| `watchdog` 配置热加载实现 | 库不同 | `notify` + `arc-swap`，行为等价 |
| `sanic-jwt` 默认的 `{"reasons":[...],"status_code":...}` 错误体 | 与统一响应体冲突 | 统一 `ApiResponse` |
| `sanic-openapi` 装饰器 | 不适用 | `utoipa` + 路由表一致性测试 |

## 5. 新增的功能

- `GET /health`：进程存活探针（Docker healthcheck 可用）。
- 请求超时（默认 30s，可关闭）→ `0001` / 408。
- 全局 panic 兜底 → `0016`，详情只进日志。
- 路径参数：`#[get("/{id}")]` + `ctx.path_param("id")`。
- 方法不允许、404 都返回统一响应体。
- `server.jwt.secret` 支持 `SANICBOOT_JWT_SECRET` 环境变量覆盖。
- 路由表自检 + OpenAPI 一致性测试，防止文档与实现漂移。

## 6. 配置迁移

| 原配置 | 处理 |
|--------|------|
| `server.http.port` | 保留 |
| `server.log.*` | 保留（`format` 支持 Python 风格占位符） |
| `server.component.*` | 删除 |
| `datasource.mysql.*` | 保留；新增可选 `acquire_timeout` |
| `redis.*` | 保留；新增可选 `db`、`password` |
| `server.http.request_max_size` | 新增 |
| `server.http.request_timeout` | 新增 |
| `server.jwt.secret` / `expires_in` | 新增（原来硬编码在 `app.py`） |
| `server.cors.allowed_origins` | 新增（默认 `["*"]`，与原来一致） |
| `server.snowflake.*` | 新增（默认 1/2，与原来一致） |
| `server.config_watch` | 新增（默认 true） |

## 7. 需要人工处理

1. **重建或升级数据库**：`password` 列加长到 `varchar(255)`，`identity` 默认值改为 3。
2. **重置密码**：Argon2id 无法校验旧 MD5；用
   `cargo run --example hash_password -- <新密码>` 生成后更新，或让用户重置。
3. **设置 JWT 密钥**：生产环境用 `SANICBOOT_JWT_SECRET`，不要用仓库里的默认值。
4. **确认路由**：若调用方使用旧文档的 `/v1/api/user/...`，需要加显式路由或改客户端。
5. **检查客户端对状态码的依赖**：业务判定改用 `code`。
6. **CORS 白名单**：生产环境把 `server.cors.allowed_origins` 改成明确域名。

## 8. 已知限制

- `%(funcName)s` 只能渲染 `event`：`tracing` 元数据没有函数名。
- 日志按天轮转；`maxBytes` 不再生效（保留配置项以兼容旧文件）。
- 一个 `#[controller] impl` 块对应一个控制器；同一结构体不要写多个 `#[controller]` impl。
- 控制器必须能通过 `#[derive(Autowired)]` 构造（通常无需手写 `new`）。
- `#[validate]` 的正则在每次请求时编译；如需极致性能可加入缓存。
- multipart 上传会把文件读入内存（与原 Python 行为一致）。
- Redis 目前只提供连接与基础命令，业务层未使用。

## 9. 迁移检查清单

- [ ] `cargo build --release` 通过
- [ ] `cargo test` 通过（75 个测试）
- [ ] 执行 `mysql/create_database.sql` 并确认种子账号能登录
- [ ] 用 `scripts/e2e-test.sh` 跑通完整用户流程
- [ ] 设置 `SANICBOOT_JWT_SECRET`
- [ ] 打开 `/swagger` 确认文档正确
- [ ] 确认调用方路由与响应码判断
