# Stemson

Stemson 是基于 **Rust + Axum** 的 API 服务骨架：用宏声明组件和路由，在启动时完成依赖注入，让业务代码集中在接口、业务逻辑和数据访问上。

项目由 [SanicBoot](https://github.com/lovemefan/SanicBoot) 的 Python/Sanic 版本重构而来，保留 Spring Boot 式的分层和约定。迁移差异见[迁移说明](docs/MIGRATION.md)。

## 它如何组织一个接口

```mermaid
flowchart LR
    H[HTTP 请求] --> C[Controller：参数与权限]
    C --> S[Service：业务逻辑]
    S --> R[Repository：SQL 与参数绑定]
    R --> D[Datasource：数据库连接与执行]
```

你声明组件之间的依赖，例如 `user_repository: Arc<UserRepository>`；框架在启动时构造组件并注入字段。请求到来后，控制器调用已经构造好的服务。

| 能力 | 写法或行为 |
|------|------------|
| 组件注册与注入 | `#[service]`、`#[repository]`、`#[datasource("mysql")]`；字段按类型注入 |
| HTTP 路由 | `#[controller("/path")]` + `#[get]` / `#[post]`；也支持从模块路径推导 |
| 参数校验与权限 | `#[validate(...)]`、`#[protected]`、`#[authority([...])]` |
| 配置 | YAML 点号取值、`#[value("key")]` 注入、文件热重载 |
| 登录 | Argon2id 密码登录或 OPAQUE 协议登录，成功后均签发 JWT |
| 数据与响应 | MySQL / PostgreSQL / SQLite 参数化查询、事务和批量操作；Redis 基础命令；统一业务响应 `{ message, data, code }` |

## 先跑起来

以下命令均在仓库根目录执行。需要 Rust 1.91+；`rust-toolchain.toml` 使用 stable 工具链。

### 1. 启动服务，验证 HTTP 链路

```bash
./scripts/dev.sh
```

脚本默认加载 `config/config.dev.yaml`，监听 **8080**。数据库均为懒连接，因此可以先不启动外部数据库，验证健康检查、Swagger 和不访问数据库的接口。

```bash
curl http://127.0.0.1:8080/health
```

预期返回 HTTP 200，响应中的 `data.status` 为 `"ok"`。这只说明服务进程可处理请求，不检查数据库连通性。

- [Swagger UI](http://127.0.0.1:8080/swagger)：浏览和调用接口。
- [OpenAPI JSON](http://127.0.0.1:8080/openapi.json)：机器可读的接口定义。

### 2. 连接 MySQL，跑通登录

用户和认证接口默认使用 MySQL 8，也可以[切换到 PostgreSQL](#使用-postgresql) 或[使用 SQLite](#使用-sqlite)。首次建库时执行：

```bash
mysql -uroot -p < mysql/create_database.sql
```

然后在 `config/config.dev.yaml` 的 `datasource.mysql` 下设置主机、端口、用户名和密码，重启服务。脚本初始化的账号为：

| 账号 | 密码 | role | identity |
|------|------|------|----------|
| `admin` | `admin123` | 1（管理员） | 1（超级管理员） |
| `lovemefan` | `lovemefan123` | 1（管理员） | 2（管理员） |

开发配置的 `server.auth.mode` 为 `both`，可以先用种子账号进行密码登录：

```bash
curl -X POST http://127.0.0.1:8080/v1/api/auth \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin123"}'
```

成功响应：

```json
{"message":"success","data":{"access_token":"...","token_type":"Bearer","expires_in":1800},"code":"0000"}
```

把 `access_token` 填入下一条请求：

```bash
curl http://127.0.0.1:8080/v1/api/auth/me \
  -H 'Authorization: Bearer <access_token>'
```

新用户的邀请注册和 OPAQUE 登录流程见 [API 文档](docs/API.md#3-认证先选登录方式再获取-jwt)。

### 也可以用 Docker 启动整套服务

```bash
docker compose up --build
```

Compose 会启动服务、MySQL 和 Redis，对外端口为 **8080**。MySQL 仅在数据卷首次初始化时运行建库脚本；已有数据卷的表结构升级需要单独处理。

### 使用 PostgreSQL

可以直接启动 PostgreSQL 16、Redis 和应用：

```bash
docker compose -f docker-compose.postgres.yml up --build
```

应用监听 **8080**，种子账号和登录方式与 MySQL 相同。PostgreSQL 数据和 OPAQUE 长期密钥均保存在命名卷中。初始化脚本仅在数据库数据卷首次创建时执行。

本地运行时，先准备可登录的 `stemson` 数据库用户，然后创建空数据库并导入表结构：

```bash
createdb -h 127.0.0.1 -U stemson stemson_backend_system
psql -h 127.0.0.1 -U stemson -d stemson_backend_system \
  -v ON_ERROR_STOP=1 -f postgres/create_database.sql
STEMSON_CONFIG=config/config.postgres.yaml cargo run
```

在 `config/config.postgres.yaml` 的 `datasource.postgres` 下填写连接信息；示例用户名和密码均为 `stemson`。现有配置也可以设置 `datasource.driver: postgres`（兼容 `postgresql`、`pgsql`），并添加同名连接配置。未设置 `driver` 时继续使用 MySQL。切换数据库后需要重启；此操作不会自动迁移已有 MySQL 数据。

自定义 Repository 可以注入 `Arc<PostgresDataSource>`，调用 `fetch_all`、`fetch_optional`、`fetch_one`、`execute`、`execute_many` 和 `begin`。PostgreSQL SQL 使用 `$1`、`$2` 占位；批量执行传入 `Vec<sqlx::postgres::PgArguments>`，支持 `None::<i64>` 等带类型的空值。详见[数据访问与事务](docs/ARCHITECTURE.md#6-数据访问与事务)。

### 使用 SQLite

SQLite 将数据库保存在本地文件中，无需启动数据库服务。在仓库根目录初始化一个空数据库后启动：

```bash
sqlite3 -bail stemson.db < sqlite/create_database.sql
STEMSON_CONFIG=config/config.sqlite.yaml cargo run
```

应用监听 **8080**，种子账号和登录方式与 MySQL 相同。初始化脚本用于空数据库，已有文件无需重复导入。`datasource.driver: sqlite` 选择 SQLite；`datasource.sqlite.path` 指定文件路径，相对路径以进程工作目录为基准，父目录需已存在。连接池可以创建缺失的文件，但不会自动建表或迁移其他数据库的数据。

文件数据库启用 WAL，默认最多 5 个连接，写锁等待超时为 5 秒。`path: ":memory:"` 使用单个连接保存内存数据库，进程退出后数据消失；普通本地运行请使用文件路径。自定义 Repository 可注入 `Arc<SqliteDataSource>`，使用 `?` / `?1` 参数绑定；批量执行接收 `Vec<sqlx::sqlite::SqliteArguments<'_>>`。

## 新增一个接口

先做一个无需数据库的例子：`GET /hello/Ada` 返回问候语。只需声明一个 Service、一个 Controller，并让 Rust 编译这两个模块。

**第一步：新增 `src/service/greeting_service.rs`。**

```rust
#[stemson::service]
pub struct GreetingService {}

impl GreetingService {
    pub fn greet(&self, name: &str) -> String {
        format!("Hello, {name}!")
    }
}
```

**第二步：新增 `src/controller/greeting_controller.rs`。**

```rust
use std::sync::Arc;

use crate::service::greeting_service::GreetingService;
use stemson::{ApiResponse, ApiResult, RequestContext};

#[derive(stemson::Autowired)]
pub struct GreetingController {
    greeting_service: Arc<GreetingService>,
}

#[stemson::controller("/hello")]
impl GreetingController {
    #[get("/{name}")]
    async fn hello(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let name = ctx.path_param("name").unwrap_or("world");
        let message = self.greeting_service.greet(name);
        Ok(ApiResponse::success(serde_json::json!({ "greeting": message })))
    }
}
```

**第三步：在各自的 `mod.rs` 中声明模块。**

```rust
// src/service/mod.rs 增加
pub mod greeting_service;

// src/controller/mod.rs 增加
pub mod greeting_controller;
```

重启服务后验证：

```bash
curl http://127.0.0.1:8080/hello/Ada
# {"message":"success","data":{"greeting":"Hello, Ada!"},"code":"0000"}
```

`pub mod` 让代码参与编译，宏负责注册和注入，无需维护中心组件列表。新增路由的 OpenAPI 描述还需要在 `src/openapi.rs` 中维护；控制器宏不会自动生成接口文档。

接入数据库时，再增加 Repository，并让 Service 注入它。SQL 放在 Repository 中，使用占位符（MySQL / SQLite 为 `?`，PostgreSQL 为 `$1`、`$2`）和 `.bind(...)` 绑定输入；完整分层和宏规则见[架构文档](docs/ARCHITECTURE.md)。

## 配置怎么选

`STEMSON_CONFIG` 指定的是一份完整配置文件，开发配置不会自动叠加到默认配置上。

| 启动方式 | 配置文件 | 端口 |
|----------|----------|------|
| `./scripts/dev.sh` | `config/config.dev.yaml` | 8080 |
| `cargo run` | `config/config.yaml` | 80 |
| `docker compose up --build` | `config/config.docker.yaml` | 8080 |
| `STEMSON_CONFIG=config/config.postgres.yaml cargo run` | `config/config.postgres.yaml` | 8080 |
| `docker compose -f docker-compose.postgres.yml up --build` | `config/config.postgres.docker.yaml` | 8080 |
| `STEMSON_CONFIG=config/config.sqlite.yaml cargo run` | `config/config.sqlite.yaml` | 8080 |

```bash
STEMSON_CONFIG=config/config.dev.yaml cargo run
```

- JWT 密钥支持 `STEMSON_JWT_SECRET` 环境变量覆盖。
- 认证模式：`password` 只允许密码登录，`both` 支持两条登录路径，`opaque` 关闭密码登录端点。
- OPAQUE 首次初始化会生成 `config/opaque_server_setup.bin`。这份长期密钥需要持久保存和备份；丢失后，原注册记录无法继续使用。默认 MySQL Compose 未为它配置持久卷；PostgreSQL Compose 已配置。
- Redis 在默认单实例配置下可选；OPAQUE 多副本防重放需要共享 Redis，并设置 `server.auth.opaque.nonce_store: redis`。
- 热重载只影响后续读取配置的逻辑。端口、连接池、JWT 和认证模式等已初始化的对象需要重启，详见[配置生效范围](docs/ARCHITECTURE.md#7-配置什么时候生效)。

## 代码在哪里

```text
src/
  main.rs / app.rs        启动、路由装配和全局中间件
  core/                   组件容器、依赖注入和路由推导
  controller/             HTTP 接口
  service/                业务逻辑
  repository/             SQL 与数据映射
  datasource/             MySQL / PostgreSQL / SQLite / Redis
  request/                请求解析与参数读取
  auth.rs / auth/         JWT / OPAQUE
  model/ / exception.rs   响应、业务码、身份模型和错误
  config.rs / openapi.rs  配置与接口文档
  utils/                  日志、密码哈希和雪花 ID
stemson-macros/            过程宏
config/                   默认、开发和 Docker 配置
mysql/                    建库脚本
postgres/                 PostgreSQL 建表与种子账号脚本
sqlite/                   SQLite 建表与种子账号脚本
scripts/ / examples/      开发、端到端测试和压测工具
tests/                    集成测试
docs/                     使用与设计文档
```

## 常用命令

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test

# 需要服务已启动，且所选 MySQL / PostgreSQL / SQLite 已初始化
STEMSON_URL=http://127.0.0.1:8080 ./scripts/e2e-test.sh

# PostgreSQL 专项集成测试（需要已初始化的临时测试库）
STEMSON_TEST_POSTGRES_CONFIG=config/config.postgres.yaml cargo test --test postgres -- --ignored

# SQLite 专项集成测试（自动创建和清理临时数据库，无需外部服务）
cargo test --test sqlite

# 生成 Argon2id 密码哈希
cargo run --example hash_password -- 'your-password'

# 对运行中的服务压测
scripts/benchmark.sh http://127.0.0.1:8080
```

## 按任务查文档

| 你想做什么 | 阅读 |
|------------|------|
| 理解启动、注入、路由和请求处理 | [架构设计](docs/ARCHITECTURE.md) |
| 调用认证与用户接口、排查业务码 | [API 文档](docs/API.md) |
| 迁移 Python 数据、客户端和配置 | [迁移说明](docs/MIGRATION.md) |
| 查看性能数据、测量口径和复现方法 | [性能对比](docs/BENCHMARK.md) |
| 了解重构保留了什么、为什么这样取舍 | [重构决策](docs/REFACTOR_PLAN.md) |

## License

MIT
