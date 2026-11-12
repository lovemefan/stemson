# SanicBoot → Stemson 重构决策

本文解释重构保留了什么，以及当前实现为什么这样选择。它按已落地代码整理，不再将早期候选方案当作待实施功能。

运行方式见 [README](../README.md)，实现细节见[架构设计](ARCHITECTURE.md)，升级步骤见[迁移说明](MIGRATION.md)。

## 1. 目标与边界

目标是将 Python/Sanic 后端模板改为 Rust/Axum，同时保留便于组织 API 服务的约定：四层结构、组件注入、自动路由、声明式校验和统一响应。

兼容的重点是可解释的业务行为与主要 URL。对密码存储、SQL 输入、错误状态和越权隐患采用明确修正，调用方与数据迁移相应调整。Python 原实现保存在 `legacy/python/`，不参与 Rust 构建。

## 2. 六个核心选择

| 问题 | 当前选择 | 原因与代价 |
|------|----------|------------|
| 怎样替代动态扫包？ | 过程宏生成注册条目，`inventory` 链接期收集 | 无需中心组件列表；新增文件仍需声明模块，组件不能在运行时动态加入 |
| 怎样构造依赖？ | 按 Datasource → Repository → Service → Controller 构造，按类型注入 `Arc<T>` | 请求期复用组件；不做依赖图排序，不解决循环依赖 |
| 怎样保持 URL？ | 保留模块路径 + 类型名推导，同时支持显式基础路径和子路径 | 旧调用方较少调整；依赖推导的路径会受模块重命名影响 |
| SQL 放在哪里？ | Repository 声明 SQL，Datasource 执行，输入使用绑定参数 | 保留原分层，移除无效黑名单；SQL 在运行时检查 |
| 怎样统一错误？ | 固定 `{ message, data, code }`，业务码用 4 位字符串，HTTP 状态表达错误类别 | 消除枚举名和值混用；旧客户端需要适配 |
| 怎样处理密码？ | Argon2id 密码登录 + 可选 OPAQUE；成功后签发 JWT | 不兼容旧 MD5，需重置；OPAQUE 需要客户端协议与长期密钥管理 |

早期曾考虑“兼容 MD5 后渐进迁移”、RequestId 中间件等方案；当前实现未采用 MD5 兼容，也未装配 RequestId 层。当前能力以代码与下表为准。

## 3. 功能落点

原 Python 路径以 `legacy/python/backend/` 为基准。

| 能力 | Python 对照 | Rust 实现 |
|------|-------------|-----------|
| YAML 点号配置、热重载 | `config/Config.py` | `src/config.rs`，`notify` + `arc-swap` |
| 配置值注入 | `core/component/value.py` | 宏的 `#[value]` 字段规则；`src/core/component/value.rs` |
| 四层注册与注入 | `core/__init__.py`、`component/autowired.py` | `stemson-macros/`、`src/core/registry.rs`、`context.rs` |
| 路由推导与注册 | `core/component/controller.py` | `stemson-macros/src/controller.rs`、`src/core/route.rs` |
| 六类参数校验 | `core/decorator/validateParameters.py` | `src/core/decorator/validate.rs`、宏的 `validate.rs` |
| JWT 与身份校验 | `sanic-jwt`、`decorator/authority.py` | `src/auth.rs`、`src/request/context.rs` |
| OPAQUE（新增） | 无 | `src/auth/opaque.rs`、`src/controller/auth_controller.rs` |
| 响应体、业务码、错误 | `model/ResponseBody.py`、`utils/StatusCode.py`、`exception/` | `src/model/response.rs`、`status_code.rs`、`src/exception.rs` |
| 中间件和错误兜底 | `app.py` | `src/app.rs`：CORS、Trace、超时、panic、404/405 |
| MySQL 查询、事务、批量 | `core/datasource/MysqlDataBasePool.py` | `src/datasource/mysql.rs`、`src/repository/` |
| Redis | `core/datasource/RedisConnectPool.py` | `src/datasource/redis.rs`：异步连接、基础命令、原子防重放占位 |
| 雪花 ID | `utils/snowflake.py` | `src/utils/snowflake.rs` |
| 密码哈希 | `utils/md5Utils.py` | `src/utils/hash.rs`：Argon2id，旧 MD5 工具移除 |
| 日志 | `utils/logger.py` | `src/utils/logger.rs`：控制台、每日文件轮转、模板兼容 |
| OpenAPI / Swagger | `sanic-openapi` | `src/openapi.rs`：`utoipa` + 内置 UI 资源 |
| 用户业务 | `controller/`、`service/`、`repository/` | `src/` 下同名三层目录 |

## 4. 选择这些依赖的目的

| 依赖 | 负责的部分 |
|------|------------|
| Axum + Tokio | 路由与异步运行时 |
| Tower / tower-http | CORS、追踪、超时与 panic 处理 |
| inventory + 自定义过程宏 | 编译生成、链接收集、启动构造 |
| serde_yaml_ng + notify + arc-swap | 配置解析、文件监听、快照切换 |
| sqlx | MySQL 连接池、参数绑定、事务和结果映射；编译无需连接数据库 |
| redis | 异步 Redis 连接和命令 |
| jsonwebtoken + argon2 + opaque-ke | JWT、密码哈希与 OPAQUE 协议 |
| utoipa + utoipa-swagger-ui | 独立维护 OpenAPI 描述并展示交互文档 |
| tracing 系列 | 日志事件、格式化、文件轮转 |

版本和完整依赖列表以 [`Cargo.toml`](../Cargo.toml) 与 `Cargo.lock` 为准。

## 5. 怎样验证这些选择

| 检查 | 覆盖范围 | 运行条件 |
|------|----------|----------|
| `cargo test` | 配置、校验、路由、认证等单元 / 集成测试 | 无需外部数据库 |
| `cargo clippy --all-targets -- -D warnings` | 静态检查 | 本地 Rust 工具链与依赖 |
| `scripts/e2e-test.sh` | 真实登录、权限与用户增删改查 | 运行中的服务 + 已初始化 MySQL |
| `examples/opaque_client.rs` | 客户端两轮注册与两轮登录 | OPAQUE 已启用、MySQL 与有效邀请 |
| `scripts/benchmark.sh` | 指定服务的端点压测 | 运行中的服务；读[性能文档](BENCHMARK.md)了解测量口径 |

测试数量随实现变化，不把某次运行的数量当作功能完整性的依据。路由表测试检查 OpenAPI 中声明的路径是否存在；它不代替全部业务场景验证。
