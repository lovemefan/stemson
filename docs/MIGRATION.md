# 从 SanicBoot 迁移到 Stemson

迁移包含三件事：**调整客户端协议、升级数据库与凭据、替换启动配置**。四层业务结构和主要 URL 保留，但密码、JWT 和错误处理不能按原样沿用。

Python 原实现位于 [`legacy/python/`](../legacy/python/)，保留原名称和内容，供对照。当前接口见 [API 文档](API.md)，实现原理见[架构设计](ARCHITECTURE.md)。

## 1. 先确认哪些地方需要改

| 主题 | Python 版 | 当前 Rust 版 | 迁移动作 |
|------|-----------|--------------|----------|
| 项目与数据库 | SanicBoot、`sanic_backend_system` | Stemson、`stemson_backend_system` | 调整启动、连接和部署配置 |
| 组件发现 | 运行时扫描包 | 过程宏 + `inventory` 链接期注册 | 声明 Rust 模块并添加组件宏，移除扫包配置 |
| 路由 | 模块路径 + 类名推导 | 保留推导规则，也支持显式路径 | 核对实际 URL，勿直接沿用旧 `user_api.md` |
| 密码 | 客户端 MD5、固定盐 | Argon2id 密码登录；可选 OPAQUE | 重置旧密码，修改客户端登录逻辑 |
| JWT | `user_id` 配置与读取不一致 | 统一 `user_id`，包含角色与身份 | 旧 token 失效，要求重新登录 |
| 响应 | `code` 混用枚举名和值 | 固定 4 位字符串业务码 | 用 `code == "0000"` 等判断业务结果 |
| HTTP 状态 | 校验、认证等状态混用 | 400/401/403/404/409 等 | 调整客户端错误处理 |
| 用户列表 | 数组放在 `message` | 数组放在 `data` | 修改取值位置 |
| SQL | 拼接输入 + 关键词黑名单 | 占位符 + 参数绑定 | 重写查询，移除黑名单依赖 |

## 2. 按顺序完成迁移

### 2.1 迁移配置和部署名称

| 项目 | 当前值 |
|------|--------|
| 应用 crate / 二进制 | `stemson` |
| 宏 crate | `stemson-macros` |
| 数据库 | `stemson_backend_system` |
| 默认 `server.name` | `stemson_backend` |
| 配置路径环境变量 | `STEMSON_CONFIG` |
| JWT 密钥环境变量 | `STEMSON_JWT_SECRET` |

早期 Rust 版本使用过的 `SANICBOOT_CONFIG`、`SANICBOOT_JWT_SECRET` 也应改为 `STEMSON_*`。

```bash
STEMSON_CONFIG=config/config.dev.yaml cargo run
```

`STEMSON_CONFIG` 选择完整文件，不进行环境配置合并。直接 `cargo run` 默认监听 80；开发脚本默认监听 8080。

| 配置 | 处理 |
|------|------|
| `server.component.*` | 删除；用模块声明和宏注册组件 |
| `datasource.mysql.*` | 继续使用；`acquire_timeout` 为连接池等待秒数 |
| `redis.*` | 继续使用；支持 `db` 和 `password` |
| `server.log.*` | 保留名称和 Python 风格模板；按天轮转，`maxBytes` 不生效 |
| `server.http.request_max_size` / `request_timeout` | 请求体字节上限 / 请求超时秒数，超时设 0 可关闭 |
| `server.jwt.secret` / `expires_in` | JWT 密钥 / 有效期；原 Python 版在代码中配置 |
| `server.auth.*` | 登录模式、OPAQUE 长期密钥、session 和防重放设置 |
| `server.cors.allowed_origins` | 默认为 `["*"]`；可设置明确域名 |
| `server.snowflake.*` | 默认 datacenter=1、worker=2，epoch 保持不变 |
| `server.config_watch` | 控制配置文件监听 |

配置热重载只改变后续读取的值，不重建连接池、JWT、认证模式或监听地址；这些改动需要重启。

### 2.2 升级数据库并重置旧密码

新建开发库可以使用 [`mysql/create_database.sql`](../mysql/create_database.sql)。已有业务数据应先备份，按脚本中的目标表结构编写增量迁移，不应把初始化脚本当成通用升级脚本。

需要核对的字段：

- `user.password`：`varchar(255)`、允许 `NULL`。Argon2id 账号存哈希；仅 OPAQUE 的账号为空。
- `user.opaque_registration`：OPAQUE 注册记录。
- `user.invite_token_hash`、`user.invite_expires_at`：邀请哈希与过期时间。
- `user.identity`：新账号默认身份为 3（普通用户），不再默认 1（超管）。

旧 MD5 摘要不能转换为 Argon2id 哈希，也不能直接校验。需要重置密码，生成新哈希后更新数据库：

```bash
cargo run --example hash_password -- 'new-password'
```

密码登录客户端改为经 HTTPS 发送密码原文，不再预先计算 MD5。原来的 `MD5 + 固定盐` 工具和 SQL 关键词过滤已移除。

### 2.3 决定是否启用 OPAQUE

| `server.auth.mode` | 登录行为 |
|--------------------|----------|
| `password` | 仅 Argon2id 密码登录 |
| `both`（仓库配置） | 两种登录都可用，便于种子账号启动和迁移 |
| `opaque` | 关闭 `POST /v1/api/auth` 密码登录，仅通过 OPAQUE 签发登录 token |

OPAQUE 将密码相关的 Argon2 计算放在客户端，服务端保存注册记录。它需要支持协议的客户端，不能只修改服务端配置。

当前模板的接入顺序是：

1. 保持 `both`，用种子管理员登录。
2. 管理员创建不带密码的用户，取得一次性邀请。
3. 客户端完成两轮注册，再完成两轮登录，确认新管理员权限可用。
4. 其他所需账号准备完成后，将模式改为 `opaque` 并重启。

现有账号没有“补发邀请 / 原地转换”接口；要迁移现有用户，需另行实现邀请签发或密码重置流程，不能只切配置。`opaque` 模式也不会删除旧哈希或移除 `validate_password` 校验接口。

部署前必须处理：

- **长期密钥**：`server.auth.opaque.setup_file` 默认指向 `config/opaque_server_setup.bin`，首次初始化生成。持久保存、备份且不要提交到版本库。丢失后既有注册记录失效；当前 Compose 没有为该文件配置持久卷。
- **多副本一致性**：使用同一数据库、同一 setup 文件和共享 Redis；设置 `nonce_store: redis`，保持 `single_use: true`。
- **重试规则**：第二轮先占用 session 再验证，失败也会消耗 session；客户端应重新发起第一轮。

单实例可用 `nonce_store: memory`，但进程重启会丢失防重放标记。协议和邀请有效期见 [API 认证说明](API.md#3-认证先选登录方式再获取-jwt)。性能数据的测量范围见[性能文档](BENCHMARK.md#8-登录opaque-与-argon2id-的差距)。

### 2.4 调整客户端路由与响应处理

以下 URL 保持 Python 当前代码的实际行为：

| 操作 | 方法与路径 |
|------|------------|
| 校验密码 | `POST /user/user_controller/validate_password` |
| 新增用户 | `POST /user/user_controller/add_user` |
| 查询用户 | `POST /user/user_controller/get_user_info` |
| 删除用户 | `POST /user/user_controller/delete_user` |
| 修改用户 | `POST /user/user_controller/modify_user` |
| 列出用户 | `POST /getall` |
| 密码登录 | `POST /v1/api/auth` |
| 当前用户 | `GET /v1/api/auth/me` |
| 校验 token | `GET /v1/api/auth/verify` |
| 续签 | `POST /v1/api/auth/refresh` |

旧 `user_api.md` 的 `/v1/api/user/...` 来自更早的 Blueprint 版本，与保留的 Python 代码不一致。若调用方依赖这些 URL，应修改客户端或增加显式别名路由。

响应字段仍为 `message`、`data`、`code`，但 `code` 统一使用字符串数值：

```json
{"message":"success","data":null,"code":"0000"}
```

不要继续比较 `code == "ADD_USER_SUCCESS"`，也不要从 `message` 读取用户列表。HTTP 状态表达传输层结果，`code` 表达具体业务原因，应结合处理。

| 场景 | 当前 HTTP | 业务码 |
|------|-----------|--------|
| 缺参数 / 参数非法 | 400 | `0010` / `0015` |
| 未认证 / token 非法 / 过期 | 401 | `0014` / `0004` / `0007` |
| 权限不足 | 403 | `0008` |
| 用户不存在 | 404 | `0013` |
| 用户名重复 | 409 | `0017` |
| 路由不存在 / 方法不允许 | 404 / 405 | `0002` |

JWT 结构调整后旧 token 需要重新签发。受保护接口默认不逐请求检查数据库状态，旧 token 的权限保留到过期；禁用账号或修改权限不会立即撤销已签发的 token。

## 3. 业务行为还有哪些变化

| 行为 | 当前实现及原因 |
|------|----------------|
| SQL 输入 | `.bind(...)` 绑定参数，合法字符串不会因包含 `select`、`or`、`%` 等被黑名单误拒 |
| 新用户身份 | `role=1` → `identity=2`；`role=0` → `identity=3`，避免创建用户默认变为超管 |
| `/getall` | 增加 JWT 和管理员 identity 检查；删除无业务意义的 `test` 参数 |
| 邮箱校验 | 接受下划线、`+` 等常见字符，不再允许旧正则误收的 竖线字符 |
| 修改用户 | HTTP 接口仍要求 6 个字段；内部 Service 命令支持可选更新，两者要区分 |
| 错误信息 | 内部错误详情写日志，对外返回通用信息 |
| 用户写入 | 创建用户与复制用户组在同一事务中执行 |

重构还修复了原实现中的遗漏 `await`、登录查询字段错误和密码校验失败分支引用未定义变量等问题。总体设计取舍见[重构决策](REFACTOR_PLAN.md)。

## 4. 验收迁移

在隔离环境中，按实际业务数据验证：

- [ ] `cargo build --release`、`cargo test` 通过。
- [ ] 数据库字段与种子 / 迁移账号可用，旧 MD5 已完成重置。
- [ ] `STEMSON_JWT_SECRET`、数据库连接、CORS 和端口符合部署配置。
- [ ] 调用方 URL、4 位业务码、用户列表位置与新 token 均已适配。
- [ ] `scripts/e2e-test.sh` 跑通登录、鉴权和用户增删改查。
- [ ] 使用 OPAQUE 时，验证注册、登录、失败重试、setup 备份与多副本防重放。
- [ ] `/health` 和 `/swagger` 可访问；另外验证 MySQL / Redis 连通性，存活探针不代替依赖检查。

## 5. 当前限制

- 一个结构体只使用一个 `#[controller] impl`；需实现 `Autowire`，通常通过 derive 完成。
- 新路由的 OpenAPI 描述需在 `src/openapi.rs` 独立维护。
- `#[validate]` 的正则按请求编译；multipart 文件全部读入内存。
- 日志按天轮转；`%(funcName)s` 只能渲染 `event`。
- OPAQUE 邀请补发、已有账号转换和密码重置没有专用 HTTP 接口。
- Docker 构建需要获取 Rust / Debian 基础镜像和构建依赖；Swagger UI 静态资源已内置，不需要单独从 GitHub 下载。
