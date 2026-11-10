# Stemson

> **Stemson**（船首内材）：从龙骨起、沿首柱向上，与内龙骨一起把船体连成整体的一根内衬材。
> 这个项目就是给服务装这样一副骨架——把分层、约定与依赖注入贯穿始终。

本项目把 [lovemefan/SanicBoot](https://github.com/lovemefan/SanicBoot) 的 Python/Sanic 后端模板
重构为 **Rust + Axum**，并更名为 **Stemson**，保留原项目「像 Spring Boot 一样快速搭建 API 服务」的设计：

- **YAML 配置**：点号取值（`server.http.port`）+ 文件热重载
- **注解式依赖注入**：`#[derive(Autowired)]` 字段注入、`#[value("...")]` 配置注入
- **自动发现 + 自动注册**：`#[service]` / `#[repository]` / `#[datasource("mysql")]` / `#[controller]`
- **自动路由**：由模块路径推导，和 Python 版 `@Controller` 完全一致
- **声明式参数校验**：`#[validate(...)]` 对应原来的 6 个校验装饰器
- **JWT 鉴权**：`#[protected]` / `#[authority([...])]`
- **统一响应体**：`{ message, data, code }` + 4 位业务码
- **数据源**：MySQL（参数化查询、事务、批量）、Redis

Python 原实现完整保留在 [`legacy/python/`](legacy/python)，可与新代码逐文件对照。

## 快速开始

### 1. 准备

- Rust 1.91+（`rustup` 安装即可，仓库带 `rust-toolchain.toml`）
- MySQL 8（可选，仅数据库相关接口需要）
- Redis（可选，当前模板未在业务中使用）

```bash
mysql -uroot -p < mysql/create_database.sql
```

脚本会创建 `stemson_backend_system` 库，并写入两个账号：

| 账号 | 密码 | 角色 | identity |
|------|------|------|----------|
| `admin` | `admin123` | 管理员 | 1（超级管理员） |
| `lovemefan` | `lovemefan123` | 管理员 | 2（管理员） |

### 2. 配置

- `config/config.yaml`：默认配置（监听 80，生产模板）
- `config/config.dev.yaml`：本地开发（监听 8080，日志 DEBUG，开热重载）
- `config/config.docker.yaml`：docker-compose 使用

数据库连接、JWT 密钥、端口都在这里改；**JWT 密钥建议用环境变量**
`STEMSON_JWT_SECRET` 覆盖。

### 3. 运行

```bash
# 本地开发（8080）
./scripts/dev.sh

# 或指定任意配置文件
STEMSON_CONFIG=config/config.yaml cargo run
```

打开：

- Swagger UI：<http://127.0.0.1:8080/swagger>
- OpenAPI JSON：<http://127.0.0.1:8080/openapi.json>
- 健康检查：<http://127.0.0.1:8080/health>

```bash
curl -X POST http://127.0.0.1:8080/v1/api/auth \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin123"}'

# {"message":"success","data":{"access_token":"...","token_type":"Bearer","expires_in":1800},"code":"0000"}
```

### 4. Docker

```bash
docker compose up --build      # Axum + MySQL + Redis，服务在 :8080
```

## 目录结构

```
Cargo.toml                 # 应用 crate（lib + bin）
stemson-macros/          # 过程宏：#\[service] #\[controller] #\[validate] ...
src/
  main.rs                  # 入口（对应 backend/app.py）
  app.rs                   # 路由装配、CORS、超时、panic/404 兜底
  config.rs                # YAML 配置 + 点号取值 + 热重载
  auth.rs                  # JWT 签发/校验、Claims
  exception.rs             # ApiError（统一错误 + HTTP 状态码映射）
  openapi.rs               # OpenAPI 文档 + Swagger UI
  core/
    registry.rs            # 组件注册表（inventory 链接期收集）
    context.rs             # AppContext：四层构建、按类型/名字解析
    route.rs               # 路由推导与路由表
    state.rs               # Axum 共享状态
    component/value.rs     # @Value 的等价实现
    decorator/validate.rs  # 6 个参数校验器
    decorator/authority.rs # 身份/权限校验
  datasource/              # MySQL、Redis 数据源
  model/                   # ApiResponse、StatusCode、User、UserIdentity、层标记 trait
  request/context.rs       # RequestContext（json/form/args/path/files 解析）
  repository/              # 数据访问（写 SQL、绑定参数）
  service/                 # 业务逻辑
  controller/              # HTTP 控制器（自动路由）
config/                    # 三套配置
mysql/create_database.sql  # 建库脚本（含 Argon2 种子密码）
scripts/                   # dev.sh、e2e-test.sh
tests/                     # 集成测试（无需数据库）
legacy/python/             # 原 Python 实现（对照用）
docs/                      # 架构、API、迁移说明
```

## 三分钟上手：新增一个接口

新增文件不需要改任何中心注册代码——宏在链接期自动注册组件。

```rust
// src/repository/article_repository.rs
use std::sync::Arc;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct ArticleRow {
    pub id: i64,
    pub title: String,
}

#[stemson::repository]
pub struct ArticleRepository {
    mysql: Arc<MysqlDataSource>,          // 自动注入同类型数据源
}

impl ArticleRepository {
    pub async fn find(&self, id: i64) -> ApiResult<Option<ArticleRow>> {
        self.mysql
            .fetch_optional(
                sqlx::query_as::<_, ArticleRow>("SELECT id, title FROM article WHERE id = ?")
                    .bind(id),            // 永远绑定参数，不要拼字符串
            )
            .await
    }
}
```

```rust
// src/service/article_service.rs
#[stemson::service]
pub struct ArticleService {
    article_repository: Arc<ArticleRepository>,
}

impl ArticleService {
    pub async fn title(&self, id: i64) -> ApiResult<Option<String>> {
        Ok(self.article_repository.find(id).await?.map(|row| row.title))
    }
}
```

```rust
// src/controller/article_controller.rs
#[derive(stemson::Autowired)]
pub struct ArticleController {
    article_service: Arc<ArticleService>,
}

#[stemson::controller("/v1/api/article")]
impl ArticleController {
    #[get("/{id}")]
    async fn detail(&self, ctx: RequestContext) -> ApiResult<ApiResponse> {
        let id: i64 = ctx
            .path_param("id")
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| ApiError::invalid_parameter("id must be a number"))?;
        let title = self.article_service.title(id).await?;
        Ok(ApiResponse::success(serde_json::json!({ "title": title })))
    }
}
```

记得在 `src/controller/mod.rs`、`src/service/mod.rs`、`src/repository/mod.rs` 里加一行 `pub mod ...;`
（Rust 需要显式声明模块；注册与注入本身是自动的）。

## 常用命令

```bash
cargo fmt --all                       # 格式化
cargo clippy --all-targets -- -D warnings
cargo test                            # 75 个单元/集成测试，无需数据库
STEMSON_URL=http://127.0.0.1:8080 ./scripts/e2e-test.sh   # 真实 MySQL 端到端
cargo run --example hash_password -- <password>             # 生成 Argon2 哈希
```

## 与原 Python 版的差异

所有有意为之的行为变化都记录在 [docs/MIGRATION.md](docs/MIGRATION.md)，重点：

| 主题 | 变化 |
|------|------|
| 路由 | 保持一致：`POST /user/user_controller/add_user` 等；另支持显式子路径 `#[get("/me")]` |
| 响应码 | `code` 统一为 4 位业务码；HTTP 状态码改为语义正确（400/401/403/404/409/500） |
| SQL | 删除关键词黑名单，全部改为参数化查询 |
| 密码 | 由 `MD5+硬编码盐` 改为 **Argon2id**，数据库需要重建/重置密码 |
| 认证 | 自行实现 JWT（不再依赖 sanic-jwt），claim 统一为 `user_id` |
| 配置扫描 | 移除 `server.component.*`（Rust 编译期注册，无需运行时扫包） |

## 文档

- [架构设计](docs/ARCHITECTURE.md)：分层、DI、路由推导、请求生命周期
- [API 文档](docs/API.md)：接口、业务码、curl 示例
- [迁移说明](docs/MIGRATION.md)：逐项差异、需要人工处理的步骤
- [重构方案](docs/REFACTOR_PLAN.md)：功能清单与设计取舍

## License

MIT，与原项目一致。
