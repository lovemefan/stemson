# API 文档

服务地址示例：`http://127.0.0.1:8080`。交互式文档：`/swagger`；机器可读：
`/openapi.json`。

## 1. 统一响应体

所有接口（含错误）都返回同一结构，字段顺序固定：

```json
{
  "message": "success",
  "data": null,
  "code": "0000"
}
```

| 字段 | 说明 |
|------|------|
| `message` | 人类可读信息；错误时为错误原因 |
| `data` | 业务数据，没有时为 `null` |
| `code` | 4 位业务码，见下表（与 HTTP 状态码相互独立） |

请求要求：

- 除文件上传外，请求体使用 `application/json`（缺失时返回 `0010` 并提示设置 Content-Type）。
- 请求体上限由 `server.http.request_max_size` 控制，默认 10 MiB，超出返回 413。
- 默认请求超时 30 秒（`server.http.request_timeout`，`0` 关闭），超时返回 `0001` / 408。

## 2. 业务码

| code | 枚举名 | HTTP | 含义 |
|------|--------|------|------|
| `0000` | SUCCESS | 200 | 成功 |
| `0001` | REQUEST_TIMEOUT | 408 | 请求超时 |
| `0002` | NOT_FOUND | 404 | 资源/路由不存在（405 也会带此码） |
| `0003` | TOKEN_AVAILABLE | 200 | token 可用 |
| `0004` | TOKEN_NOT_AVAILABLE | 401 | token 非法/不可用 |
| `0005` | USERNAME_OR_PASSWORD_EMPTY | 400 | 用户名或密码为空 |
| `0006` | USERNAME_OR_PASSWORD_ERROR | 401 | 用户名或密码错误 |
| `0007` | TOKEN_TIMEOUT | 401 | token 过期 |
| `0008` | PERMISSION_DENIED | 403 | 身份不足 |
| `0009` | ADD_USER_FAILED | 400 | 添加用户失败（如 role 非 0/1） |
| `0010` | MISS_PARAMETERS | 400 | 缺少参数 |
| `0011` | DELETE_USER_FAILED | 400 | 删除用户失败 |
| `0012` | MODIFY_USER_FAILED | 400 | 修改用户失败 |
| `0013` | USER_NOT_EXIST | 404 | 用户不存在 |
| `0014` | UNAUTHORIZED | 401 | 未认证（缺少 Authorization） |
| `0015` | INVALID_PARAMETER | 400 | 参数非法 |
| `0016` | INTERNAL_SERVER_ERROR | 500 | 服务内部错误（详情只写日志） |
| `0017` | USER_ALREADY_EXIST | 409 | 用户名已存在（原文档提到、原枚举缺失，本次补上） |

## 3. 认证

### 3.1 登录

```http
POST /v1/api/auth
Content-Type: application/json

{"username": "admin", "password": "admin123"}
```

```json
{
  "message": "success",
  "data": {
    "access_token": "eyJ0eXAiOiJKV1Qi...",
    "token_type": "Bearer",
    "expires_in": 1800
  },
  "code": "0000"
}
```

错误：`0010`（缺参数）、`0013`（用户不存在）、`0006`（密码错误）。

### 3.2 当前用户

```http
GET /v1/api/auth/me
Authorization: Bearer <token>
```

```json
{
  "message": "success",
  "data": {
    "me": {
      "user_id": 1341983140255768576,
      "username": "admin",
      "phone": null,
      "email": null,
      "role": 1,
      "create_by": "admin",
      "create_time": "2020-12-24 13:37:38",
      "last_login_time": "2026-11-10 14:40:03",
      "status": 1
    }
  },
  "code": "0000"
}
```

### 3.3 校验 token

```http
GET /v1/api/auth/verify
Authorization: Bearer <token>
```

```json
{"message":"success","data":{"valid":true,"user_id":1341983140255768576,"username":"admin","expires_at":1794294556},"code":"0000"}
```

### 3.4 续签

```http
POST /v1/api/auth/refresh
Authorization: Bearer <token>
```

返回与登录相同的 `TokenResponse`（`access_token` 为新的 token）。

### 3.6 OPAQUE 登录（推荐）

`server.auth.mode` 为 `opaque` 或 `both` 时可用。登录拆成两轮，密码永远不出现在
服务端；服务端只存 OPAQUE 注册记录（`user.opaque_registration`），不存哈希。

**第一轮**

```bash
curl -X POST http://127.0.0.1:8080/v1/api/auth/login/start \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","credential_request":"<base64url>"}'
```

```json
{
  "message": "success",
  "data": {
    "credential_response": "<base64url>",
    "session": "<base64url，服务端加密状态，客户端原样回传>"
  },
  "code": "0000"
}
```

**第二轮**

```bash
curl -X POST http://127.0.0.1:8080/v1/api/auth/login/finish \
  -H 'Content-Type: application/json' \
  -d '{"session":"<上一轮返回>","credential_finalization":"<base64url>"}'
```

返回与 3.1 相同的 `access_token`。密码错误返回 `0006` / 401；`session` 过期（默认
120 秒，`server.auth.opaque.session_ttl`）或**已被使用过**返回 `0014` / 401。

严格一次性：`server.auth.opaque.single_use`（默认 `true`）开启后，第二轮会在校验
`CredentialFinalization` 之前用 `SET NX EX` 原子占位，因此同一个 `session` 即使被
并发提交也只能成功一次。单实例用 `nonce_store: memory` 即可；多副本部署必须用
`nonce_store: redis`（共享 Redis，TTL 与 `session_ttl` 一致）。

`session` 是**无状态**的：里面是加密后的 `ServerLogin` 状态和过期时间，任何持有同一
份 `setup_file` 的实例都能完成第二轮，因此 OPAQUE 登录可以水平扩展，不需要 Redis
或粘性会话。

### 3.7 OPAQUE 注册（邀请制）

管理员创建用户时不传密码，服务端返回一次性 `invite_token`（24 小时有效、
只能使用一次）：

```bash
# 管理员登录后
curl -X POST http://127.0.0.1:8080/user/user_controller/add_user \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -d '{"username":"alice","role":0,"email":"alice@example.com"}'
```

```json
{
  "message": "Add user alice success",
  "data": {
    "user_id": 2120078728141873152,
    "username": "alice",
    "invite_token": "xRTDrB8-tLJQwoGsoEVAJFnwWyPE6XKZ3bVtXPNJR0k",
    "registration_required": true
  },
  "code": "0000"
}
```

用户拿到邀请后完成注册：

```bash
# 第一轮
curl -X POST http://127.0.0.1:8080/v1/api/auth/register/start \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","registration_request":"<base64url>"}'

# 第二轮
curl -X POST http://127.0.0.1:8080/v1/api/auth/register/finish \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","invite_token":"<上面返回的>","registration_upload":"<base64url>"}'
```

注册成功后该账号的 `password` 置空，只能用 OPAQUE 登录。

**客户端实现**：浏览器/Node 用
[`@serenity-kit/opaque`](https://github.com/serenity-kit/opaque)（后端同一套
`opaque-ke` 的 wasm 版）；Rust 客户端示例见 `examples/opaque_client.rs`：

```bash
cargo run --release --example opaque_client -- register \
  --url http://127.0.0.1:8080 --username alice --password 's3cret' --invite TOKEN
cargo run --release --example opaque_client -- login \
  --url http://127.0.0.1:8080 --username alice --password 's3cret'
```

服务端成本约 **0.23 ms/次登录**（Argon2id 校验约 14.8 ms），详见
[docs/BENCHMARK.md](BENCHMARK.md)。

### 3.8 鉴权规则

- 缺少 `Authorization` → `0014`；格式不是 `Bearer <token>` → `0014`；
- token 伪造/损坏 → `0004`；过期 → `0007`；
- 账户被禁用（`status=0`）→ `0008`。

## 4. 用户接口

> 路由保持 Python 版的实际行为（由模块路径 + 类名推导），并非旧文档里的
> `/v1/api/user/...`；详见 `MIGRATION.md`。

### 4.1 校验用户名密码

```http
POST /user/user_controller/validate_password
Content-Type: application/json

{"username": "admin", "password": "admin123"}
```

- 密码长度 5–20（不满足 → `0015`，消息 `Length of password must be 5 to 20`）。
- 正确 → `0000`，`data.valid = true`；错误 → `0006`。

### 4.2 新增用户（管理员）

```http
POST /user/user_controller/add_user
Authorization: Bearer <admin token>

{"username": "test2", "password": "secret123", "role": 0,
 "phone": "18600000000", "email": "test2@example.com"}
```

- 需要 `role != 0`（管理员）；普通用户 → `0008`。
- `role` 必须为 0/1，否则 `0009`。
- 用户名重复 → `0017`。
- 成功返回新用户（不含密码）。

新用户的 `identity`：`role=1` → 2（管理员）；`role=0` → 3（普通用户）。
原实现依赖列默认值 1（超管），属于越权隐患，这里已修正。

### 4.3 查询用户（管理员）

```http
POST /user/user_controller/get_user_info
Authorization: Bearer <admin token>

{"user_id": 1342014866998829056}
```

也支持 `{"username": "lovemefan"}`；都没有时返回 `0010`（`username is empty`）。

### 4.4 修改用户（管理员）

```http
POST /user/user_controller/modify_user
Authorization: Bearer <admin token>

{
  "user_id": 1342014866998829056,
  "username": "lovemefan",
  "email": "lovemefan@outlook.com",
  "password": "new-secret-123",
  "user_role": 1,
  "status": 1
}
```

- 6 个字段必填（与 Python 版一致）；`email` 需通过格式校验（失败 → `0015`）。
- `password` 传明文，服务端用 Argon2id 重新哈希。

### 4.5 删除用户（管理员）

```http
POST /user/user_controller/delete_user
Authorization: Bearer <admin token>

{"user_id": 1342014866998829056}
```

### 4.6 列出全部用户（管理员）

```http
POST /getall
Authorization: Bearer <admin token>
```

```json
{
  "message": "success",
  "data": [
    {"user_id": 1341983140255768576, "username": "admin", "role": 1, "...": "..."}
  ],
  "code": "0000"
}
```

身份不足（identity 低于 ADMIN）→ `0008`。

## 5. 运维接口

```http
GET /health
```

```json
{"message":"success","data":{"status":"ok","name":"stemson_backend","uptime_seconds":12,"components":11},"code":"0000"}
```

进程存活即 200；MySQL/Redis 不可用不影响该探针（数据源懒连接）。

## 6. 路径参数

`#[get("/{id}")]` 风格的路径参数可通过 `ctx.path_param("id")` 或
`ctx.param("id", ParamScope::Path)` 读取；`all` 作用域也会合并它们
（优先级低于 body、form、query）。
