# 架构设计

本文说明 Rust/Axum 版如何对应 Python 原实现，以及请求在框架中的完整生命周期。

## 1. 分层

```
HTTP 请求
   │
   ▼
Axum Router ──(CORS / Trace / Timeout / CatchPanic)──┐
   │                                                  │
   ▼                                                  │
生成的 handler（每个 #[get]/#[post] 一个）             │
   │  1. 解析 RequestContext（json/form/args/path/files + JWT）│
   │  2. #[protected] / #[authority] 鉴权              │
   │  3. #[validate] 参数校验                          │
   ▼                                                  │
Controller（从容器取出，字段已注入）                    │
   ▼                                                  │
Service                                               │
   ▼                                                  │
Repository（声明 SQL + 绑定参数）                       │
   ▼                                                  │
Datasource（MySQL 连接池 / Redis）                     │
```

四层组件的注册顺序与 Python 的注入链一致：
**datasource → repository → service → controller**。

## 2. 组件注册与依赖注入

### 2.1 注册表

`src/core/registry.rs` 定义 `ComponentRegistration`，宏为每个组件生成一条：

```rust
inventory::submit! {
    ComponentRegistration {
        name: "user_service",
        kind: ComponentKind::Service,
        type_name: "UserService",
        build: __stemson_build_user_service,
        as_controller: None,
    }
}
```

`inventory` 在**链接期**收集所有条目，因此：

- 新增组件只需写宏，不用改任何中心注册代码（对应 Python 的自动扫包注册）；
- 无需运行时反射或包扫描（Rust 没有等价能力，见 `MIGRATION.md`）。

启动时 `AppContext::build_components()` 按层构建，同层按名字排序，保证启动确定性；
重名（同层）或同类型重复注册会直接报错。

### 2.2 字段注入

`#[derive(Autowired)]` / `#[service]` / `#[repository]` / `#[datasource]` /
`#[controller]` 都会解析结构体字段并生成 `Autowire::autowire(ctx)`：

| 字段写法 | 注入内容 |
|----------|----------|
| `foo: Arc<SomeComponent>` | 从容器按类型取 `SomeComponent` |
| `config: Arc<Config>` | 应用配置（`ctx.config().clone()`） |
| `#[value("datasource.mysql.port")] port: u16` | 配置项反序列化为 `u16`，缺失即启动失败 |
| `#[skip] cache: OnceLock<T>` | `Default::default()`，用于运行期状态 |

对应关系：

```python
@Service
class UserService(ServiceBase):
    @Autowired
    def user_repository(self):
        pass
```

```rust
#[service]
pub struct UserService {
    user_repository: Arc<UserRepository>,
}
```

Python 里「被注入的方法不能带 `self`」的限制自然消失了。

### 2.3 按名字解析

容器同时保留 Python 的「小写下划线名字」索引（`user_service`、`mysql`），
`AppContext::resolve_by_name("mysql")` 可取到任意组件，便于调试与兼容脚本。
名字只在同一层内唯一（与 Python 的四张独立字典一致）。

## 3. 自动路由

### 3.1 推导规则

与 `backend/core/component/controller.py` 完全一致：

```text
路径 = "/" + snake(模块路径去掉 <crate>::controller 前缀) + "/" + snake(结构体名)
```

| 控制器（文件 `src/controller/user/user_controller.rs`） | 路由 |
|----------------------------------------------------------|------|
| `ValidatePassword` | `POST /user/user_controller/validate_password` |
| `AddUser` | `POST /user/user_controller/add_user` |
| `GetUserInfo` | `POST /user/user_controller/get_user_info` |
| `DeleteUser` | `POST /user/user_controller/delete_user` |
| `ModifyUser` | `POST /user/user_controller/modify_user` |
| `GetAllInfo`（`#[controller("/getall")]`） | `POST /getall` |

显式路径会覆盖推导结果；`#[get("/me")]` 这类子路径会与基础路径拼接：

```rust
#[controller("/v1/api/auth")]      // GET /v1/api/auth/me
impl AuthController {
    #[get("/me")]
    async fn me(&self, ctx: RequestContext) -> ApiResult<ApiResponse> { ... }
}
```

HTTP 方法由 `#[get]` `#[post]` `#[put]` `#[delete]` `#[patch]` `#[head]` `#[options]` 声明，
取代了 `HTTPMethodView` 的 `post`/`get` 方法名。

### 3.2 路由表

每个控制器同时生成 `route_entries()`，装配路由时汇总到 `AppContext`，
用于：

- 启动日志/自检；
- OpenAPI 一致性测试（`tests/router.rs` 断言文档里的每个路径都真实存在）。

## 4. 请求生命周期

以 `AddUser` 为例，宏生成的 handler 等价于：

```rust
async fn __stemson_route_add_user_0(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let result: ApiResult<ApiResponse> = async move {
        let ctx = RequestContext::from_request(&state, request).await?;
        ctx.require_auth()?;                          // #[protected]
        // ctx.require_identity(&[...])?;             // #[authority([...])]
        {
            let mut v = ctx.validator(ParamScope::Json);   // #[validate(...)]
            v.not_empty(&["username", "password"]);
            v.check()?;
        }
        let controller = state.ctx().resolve::<AddUser>()?;
        AddUser::post(&*controller, ctx).await
    }.await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => error.into_response(),
    }
}
```

要点：

1. **请求体只读一次**：`RequestContext` 在构造时把 body 读入内存并按 `Content-Type`
   解析为 `json` / `form` / 文件；字段访问统一走 `ctx.text()`、`ctx.json_as::<T>()`、
   `ctx.param(key, scope)`。
2. **鉴权先于校验**，与 Python 装饰器栈 `@protected` 在 `@NotEmpty` 外侧的顺序一致。
3. 控制器实例由容器提供，字段注入发生在启动阶段，请求期只做一次哈希表查找。
4. 所有错误都转成 `ApiError`，最终渲染为统一响应体。

## 5. 参数校验

| Python 装饰器 | Rust 写法（`#[validate(...)]`） |
|---------------|----------------------------------|
| `@NotEmpty(required=[...], parameter_type=...)` | `not_empty(["..."])` |
| `@Length(key, min, max, message)` | `length("key", min, max, "message")` |
| `@Range(key, min, max, message)` | `range("key", min, max, "message")` |
| `@Pattern(key, pattern, pattern_mode, message)` | `pattern("key", r"...", match, "message")` |
| `@EnumString(key, value=[...], message)` | `enum_string("key", ["a", "b"], "message")` |
| `@Assert(key, condition, message)` | `assert("key", fn_name, "message")` |
| 文件断言（鸭子类型） | `assert_file("key", fn_name, "message")` |

- 作用域：`json` / `form` / `args` / `path` / `files` / `all`（默认 `all`，
  合并优先级 path → form → args → files → json）。
- 语义逐条对齐原实现：缺少参数 → `MISS_PARAMETERS(0010)`，值非法 → `INVALID_PARAMETER(0015)`；
  第一个失败即返回（与装饰器链一致）。
- `pattern` 的 `match` 等价 Python `re.match`（从行首锚定），`search` 等价 `re.search`。

## 6. 认证与鉴权

- `Claims`：`user_id`、`username`、`role`、`identity`、`scopes`、`iat`、`nbf`、`exp`。
  原实现把 JWT 的 `user_id` 配置成 `username`，却按 `user_id` 读取，导致 `/me`
  与 `@protected` 取不到用户；这里统一为 `user_id`（见 `MIGRATION.md`）。
- 每次请求都会尝试解析 `Authorization: Bearer <token>`：
  - 合法 → `ctx.auth = Some(AuthUser)`；
  - 非法 → 记录原因，`require_auth()` 返回具体错误（`0004` 或 `0007`）；
  - 缺失 → `0014`。
- `#[protected]`：要求已认证；`#[code]` 处理器内部可 `ctx.require_user()?` 取 Claims。
- `#[authority([UserIdentity::Admin])]`：按身份做「≥」比较（超管满足管理员，管理员满足普通用户）。
- 业务层还保留原实现的 `role` 判断：`ctx.require_admin("add user information")`。

## 7. 数据源

### MySQL

- `sqlx::MySqlPool`，**懒连接**：服务可以在 MySQL 暂时不可用时启动（与原 `aiomysql` 一致）。
- Repository 声明 SQL，数据源负责执行与映射：

```rust
let sql = format!("{SELECT_USER} WHERE u1.uid = ?");
self.mysql
    .fetch_optional(sqlx::query_as::<_, UserRow>(&sql).bind(uid))
    .await
```

- `execute_many`：同一语句批量执行（对应 `cursor.executemany`），内部包事务。
- `begin()`：拿到 `sqlx::Transaction`，用于「插入用户 + 复制用户组」这类原子操作。
- 无符号列（`BIGINT UNSIGNED` / `TINYINT UNSIGNED`）在 SELECT 中 `CAST(... AS SIGNED)`，
  Rust 侧保持 `i64`/`i32`，与 JWT、API 一致。

### Redis

`RedisDataSource` 持有 `ConnectionManager`（懒连接），提供 `get/set/set_ex/del/get_json/ping`。

## 8. 配置

- `Config::get("a.b.c")` 逐段下钻；命中标量则返回标量（与 Python 语义一致）。
- 类型化读取：`string/i64/u16/bool/get_as/require`。
- 注入：`#[value("...")]` 在启动期读取，缺失直接失败（相当于 Python 抛 `ValueError`）。
- 热重载：`notify` 监听配置文件，`arc-swap` 原子替换，读路径无锁；
  200 ms 去抖以合并编辑器连续写入。
  **只有「每次读取时取值」的配置会生效**（例如 `/health` 里的 name、校验开关）；
  数据库连接、监听端口等基础设施配置需要重启。

## 9. 日志

- `tracing` + 自定义 `FormatEvent`，解析原配置里的 Python 风格模板：
  `%(asctime)s %(levelname)s %(filename)s %(lineno)d %(module)s %(funcName)s %(message)s`。
- 控制台带颜色；文件按**天**轮转，`backupCount` 控制保留个数。
- `%(funcName)s` 无法精确对应：`tracing` 元数据里没有函数名，事件名形如
  `event <file>:<line>`，这里只保留 `event`。

## 10. 扩展点

| 想做的事 | 做法 |
|----------|------|
| 新增数据源 | 实现 `#[datasource("name")]` 结构体 + `DatasourceBase`，Repository 注入它 |
| 新增校验规则 | 在 `Validator` 加方法，在宏的 `validate.rs` 加一个 rule 分支 |
| 新增中间件 | 在 `app.rs::build_router` 的层叠处添加 `tower` 层 |
| 自定义 404/500 | 改 `app.rs` 的 `not_found` / `CatchPanicLayer` |
| 关闭请求超时 | `server.http.request_timeout: 0` |
