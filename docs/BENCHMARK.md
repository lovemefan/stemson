# 性能对比：Stemson (Rust/Axum) vs SanicBoot (Python/Sanic)

在**同一台机器、同一个 MySQL、同一个压测客户端**下对比 `legacy/python`（原 Sanic 实现）
与新 Stemson 的吞吐和延迟。

## 1. 结论

| 场景 | 说明 | 单核 (1 P-core) | 多核 (8 P-core) |
|------|------|-----------------|-----------------|
| `404` | 路由未命中 + JSON 错误体，无数据库 | **4.6x** | **7.6x** |
| `validate` | 参数校验失败，无数据库 | **3.4x** | **7.0x** |
| `getall` | 全表 `LEFT JOIN` + JSON 序列化 | **9.8x** | **18.0x** |
| `login` | 认证（算法不同，见 §5.3） | **0.14x（慢 7 倍）** | **0.37x（慢 2.7 倍）** |

一句话：**纯框架路径快 3–5 倍/核，数据库路径快约 10 倍/核；只有登录更慢，
因为 Rust 版把 MD5 字符串比较换成了 Argon2id 密码哈希。**

## 2. 测试环境

| 项 | 值 |
|----|----|
| CPU | Intel Core i9-14900HX，8 个 P-core（16 线程）+ 16 个 E-core，32 逻辑核 |
| 内存 | 31 GiB |
| 系统 | Ubuntu 22.04 (kernel 6.x) |
| Rust | 1.99.1，`cargo build --release` |
| Python | 3.9.20，Sanic 22.12.0，uvloop 0.17.0，aiomysql 0.1.1 |
| MySQL | 8.4.5，Docker 容器，`--max-connections=1000` |
| 压测客户端 | 仓库内 `examples/loadgen.rs`（reqwest，HTTP/1.1 keep-alive） |
| 数据量 | 两张库各 2 个用户（`LEFT JOIN user AS u2`） |

两套库来自各自的建表脚本：`sanic_backend_system`（原 MD5 种子）与
`stemson_backend_system`（Argon2id 种子）。

## 3. 方法

- **同一客户端**压两端，避免工具差异；客户端不参与被测进程的 CPU 争抢。
- 两种 CPU 配置：
  - **单核**：服务端绑到 1 个 P-core（CPU 0），客户端用其余 31 个 CPU；
  - **多核**：服务端绑到 8 个 P-core 的物理线程（CPU 0,2,4,…,14），客户端用剩余 24 个 CPU。
- 并发：单核 32，多核 64；每轮 2s 预热 + 8s 测量，重复 3 次取中位数。
- 已排除的干扰：
  - 关闭 Sanic 的 `ACCESS_LOG`（Stemson 的 `TraceLayer` 在 WARN 级别不输出每请求日志），
    否则日志 I/O 会主导对比；
  - MySQL 直接从容器内网 IP 访问，绕开 Docker 端口代理（`docker-proxy` 在 64+ 并发
    短连接下会偶发断连，曾让两端都出现 5xx）；
  - 两端 MySQL 连接池都是 100；数据库 `max_connections` 提到 1000，避免互相挤占。

### 场景

| 场景 | Stemson | SanicBoot |
|------|---------|-----------|
| 404 | `GET /nope` | `GET /nope` |
| validate | `POST /user/user_controller/validate_password` `{"username":"x"}` | 同 |
| getall | `POST /getall` + Bearer JWT | `POST /getall` `{"test":1}`（原版此接口无鉴权） |
| login | `POST /v1/api/auth` `admin/admin123` | `POST /v1/api/auth` `lovemefan/<md5>` |

## 4. 结果

### 4.1 单核（1 个 P-core）

| 场景 | Sanic RPS | Stemson RPS | 倍数 | Sanic p99 | Stemson p99 |
|------|-----------|-------------|------|-----------|-------------|
| 404 | 17,145 | **79,598** | 4.6x | 4.05 ms | 0.96 ms |
| validate | 19,571 | **67,359** | 3.4x | 2.59 ms | 1.20 ms |
| getall | 1,352 | **13,233** | 9.8x | 62.32 ms | 3.59 ms |
| login | **452** | 63 | 0.14x | 145.2 ms | 502.4 ms |

### 4.2 多核（8 个 P-core）

| 场景 | Sanic RPS | Stemson RPS | 倍数 | Sanic p99 | Stemson p99 |
|------|-----------|-------------|------|-----------|-------------|
| 404 | 17,895 | **135,023** ¹ | 7.6x | 4.59 ms | 1.13 ms |
| validate | 18,239 | **128,121** ¹ | 7.0x | 11.06 ms | 1.45 ms |
| getall | 1,524 | **27,466** | 18.0x | 83.37 ms | 5.44 ms |
| login | **484** | 177 | 0.37x | 215.2 ms | 572.2 ms |

¹ 单客户端在 135k 时已打满自身 CPU。用两个客户端分别绑不同 CPU 并同时压，
合计 **160,559 RPS**，说明服务端仍有富余；这个数字是**下限**。

Sanic 的 1 核与 8 核结果几乎一致（17.1k → 17.9k）：单进程单线程，多核不扩展。
Stemson 从 1 核 79.6k 提升到 8 核 ≥135k，随核数扩展（受客户端限制未测到上限）。

## 5. 分析

### 5.1 框架路径（404 / 校验）3.4–4.6x/核

处理链路相同：解析 HTTP → 路由匹配 → 参数校验 → 序列化统一响应体。
差距来自解释执行 vs 原生代码：Sanic 每个请求都要经过 Python 层（uvloop 回调、
`Request` 对象、装饰器链、`dict`/`json` 序列化），Stemson 是编译后的状态机 +
`serde_json`；单核 17k vs 80k 正好反映这一点。

### 5.2 数据库路径差距更大（9.8x/核，多核 18x）

`getall` 每次请求都要：

1. 取连接（池）
2. 执行 `SELECT ... LEFT JOIN`
3. 行 → 对象 → JSON

Stemson 只做 1+2+3 的最小集（`sqlx` 行解码到 `UserRow`，`to_dict` 一次序列化）；
Sanic 侧 `aiomysql` + PyMySQL 协议实现 + 每行的 Python `tuple` 解包 + `User`
对象构造 + `__dict__` 序列化，单位请求的 Python 开销显著高于往返数据库本身。
所以差距比纯框架路径更大。

另外 Stemson 的响应体更小：`to_dict()` 不返回 `identity` / `user_identity`
（原 Python 版会带），这部分也少了一点序列化成本。

### 5.3 登录慢：安全成本的体现，不是框架退步

| | SanicBoot | Stemson |
|---|---|---|
| 客户端 | 先做 MD5，再发送摘要 | 发送明文（需 TLS） |
| 服务端 | 字符串比较 | **Argon2id**（m=19 MiB, t=2, p=1） |
| 单次成本 | ~微秒 | ~16 ms CPU + 19 MiB 内存带宽 |

单核 63 RPS ≈ 每核 16 ms/次，正好是 Argon2id 的代价；8 核只到 177 RPS ≈ 2.8 核
等效，因为 Argon2 是**内存带宽密集**型，扩展性有限。原版 MD5 方案的问题是：
无盐/弱盐、可彩虹表、客户端摘要可重放，不能作为生产方案。

如果登录吞吐是瓶颈，可调 `Argon2` 参数（降低 `m`、保持 `t=2`）或换成
Argon2id + 短时 token 缓存；安全性与吞吐需要按业务权衡。

### 5.4 尾延迟

除登录外，Stemson 的 p99 全面更低（0.96–5.44 ms vs 2.59–83.37 ms）。
Sanic 的 `getall` p99 达到 62–83 ms，主要来自单线程排队；Rust 版 3.6–5.4 ms。

## 6. 复现

```bash
# 1) MySQL（两套库一起初始化）
mkdir -p .bench/mysql
cp legacy/python/mysql/create_database.sql .bench/mysql/01-sanic.sql
cp mysql/create_database.sql              .bench/mysql/02-stemson.sql
docker run -d --name bench-mysql -e MYSQL_ROOT_PASSWORD=root -p 13306:3306 \
  -v "$PWD/.bench/mysql:/docker-entrypoint-initdb.d:ro" \
  mysql:8 --character-set-server=utf8mb4 --collation-server=utf8mb4_bin --max-connections=1000
IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' bench-mysql)

# 2) 原 Sanic 版（Python 3.9 + requirements）
python3 -m venv .bench/venv
.bench/venv/bin/pip install -r legacy/python/backend/requirements.txt cryptography
# 配置：datasource.mysql.host=$IP，port=3306；顶层 http.port=18000；
#      关闭 app.config.ACCESS_LOG（仅基准用）
cd legacy/python && PYTHONPATH=. ../../.bench/venv/bin/python backend/app.py

# 3) Stemson
STEMSON_CONFIG=.bench/stemson.yaml cargo run --release

# 4) 压测（单核示例：服务端绑 CPU0，客户端用其余核）
cargo build --release --example loadgen
taskset -c 1-31 ./target/release/examples/loadgen \
  --url http://127.0.0.1:18001/nope --concurrency 32 --duration 8
```

四个场景的完整命令见 `scripts/benchmark.sh`。

## 7. 局限

- 同机测试，无真实网络延迟；MySQL 与压测端共享 CPU/内存带宽。
- 数据量很小（每库 2 行），`LEFT JOIN` 成本极低；数据量增大后数据库路径的差距会变化。
- 单核/多核划分依赖本机 P-core/E-core 拓扑，其他机器绝对值会不同，**倍数更有参考意义**。
- Sanic 的 `getall` 无鉴权，Stemson 需要 JWT：Rust 侧多了 HMAC 校验（<1 µs），影响可忽略但存在。
- 登录场景算法不同（MD5 vs Argon2id），只能说明**产品级端点**的差异，不是框架差异。
- 为了让原版在 Python 3.9 + MySQL 8.4 下跑起来，基准副本做了三处**环境性**调整（不改业务代码）：
  1. 安装 `cryptography`（PyMySQL 连 MySQL 8 的 `caching_sha2_password` 需要）；
  2. 配置里补顶层 `http.port`（原 `app.py` 读的是 `http.port` 而不是 `server.http.port`）；
  3. 关闭 `ACCESS_LOG`。

## 8. 登录：OPAQUE 与 Argon2id 的差距

上表里的登录场景是 Argon2id（`m=19 MiB, t=2`）。换成 OPAQUE 后，服务端只做一次
OPRF 和一次密钥交换校验，客户端承担 Argon2 KSF。

**单核、进程内测量**（`cargo run --release --example opaque_cost`）：

| 步骤 | 单次成本 |
|------|----------|
| OPAQUE `login/start`（OPRF + 会话加密） | 229 µs |
| OPAQUE `login/finish`（MAC 校验 + 签发 JWT） | 2 µs |
| **OPAQUE 合计** | **231 µs** |
| Argon2id 校验 | 14,788 µs |
| Argon2id 生成哈希 | 13,266 µs |

即单核登录容量从 **68 次/秒** 提升到 **约 4,300 次/秒**（约 64 倍），而安全属性
更强：密码不出客户端，拖库后没有 `setup_file` 无法离线爆破。

**HTTP 层（单 P-core，c=32）**：

| 端点 | RPS | p50 | p99 |
|------|-----|-----|-----|
| `POST /v1/api/auth/login/start` | 5,668 | 5.61 ms | 7.56 ms |
| `POST /v1/api/auth/login/finish` | 8,450 | 3.63 ms | 5.92 ms |

一次完整登录需要两轮，按串联估算单核约 **3,400 次登录/秒**；作为对比，Argon2id
密码登录在同样条件下是 **63 次/秒**（p99 697 ms）。这里的 HTTP 数字复用了预先
构造的合法消息，只衡量服务端计算，不含客户端 Argon2。

**代价**：客户端（浏览器）仍要花 ~16 ms 做 KSF，所以单次登录的端到端时延主要由
客户端 CPU 决定；服务端容量则提升两个数量级。这正是 aPAKE 的设计意图。
