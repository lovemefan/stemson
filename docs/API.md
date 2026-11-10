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

### 3.5 鉴权规则

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
{"message":"success","data":{"status":"ok","name":"sanic_backend","uptime_seconds":12,"components":11},"code":"0000"}
```

进程存活即 200；MySQL/Redis 不可用不影响该探针（数据源懒连接）。

## 6. 路径参数

`#[get("/{id}")]` 风格的路径参数可通过 `ctx.path_param("id")` 或
`ctx.param("id", ParamScope::Path)` 读取；`all` 作用域也会合并它们
（优先级低于 body、form、query）。
