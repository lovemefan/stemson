# 性能对比：Stemson (Rust/Axum) vs SanicBoot (Python/Sanic)

本文保留仓库已有的基准记录，对比 `legacy/python` 与 Rust 服务在同一机器、MySQL 和压测客户端下的端点吞吐与延迟。本次文档整理未重新测量这些数据。

先区分三种口径：

| 口径 | 回答的问题 | 位置 |
|------|------------|------|
| HTTP 端点吞吐 / 延迟 | 指定并发和环境下，整个端点能处理多少请求？ | 第 1–7 节 |
| 进程内协议计算 | OPAQUE / Argon2 单次服务端操作的计算量是多少？ | 第 8 节 |
| 完整登录体验 | 客户端计算 + 网络两轮 + 数据库 + 服务端，总共多久？ | 本文没有完整端到端测量 |

RPS 表示每秒请求数，p99 表示 99% 请求的耗时不超过该值。不同口径的数字不能直接互换。

## 1. 结论

| 场景 | 说明 | 单核 (1 P-core) | 多核 (8 P-core) |
|------|------|-----------------|-----------------|
| `404` | 路由未命中 + JSON 错误体，无数据库 | **4.6x** | **7.6x** |
| `validate` | 参数校验失败，无数据库 | **3.4x** | **7.0x** |
| `getall` | 全表 `LEFT JOIN` + JSON 序列化 | **9.8x** | **18.0x** |
| `login` | 认证（算法不同，见 §5.3） | **0.14x（慢 7 倍）** | **0.37x（慢 2.7 倍）** |

在这组端点测试中，Rust 单核的 404 / 校验吞吐为 Python 的 3.4–4.6 倍，用户列表为 9.8 倍。密码登录的工作量不同：Rust 执行 Argon2id，Python 只比较客户端 MD5 摘要，不能用该场景判断框架本身快慢。

这里的“8 核”是分配的 CPU 资源。Python 使用单 worker，Rust 使用 Tokio 多线程；它不是 Sanic 多 worker 与 Rust 的完整部署对比。

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

### 5.1 框架路径：单核吞吐差异

404 和参数校验失败都不访问数据库。Rust 在这两条路径的单核吞吐分别为 Python 的 4.6 倍和 3.4 倍。

两端都包含 HTTP 解析、路由和 JSON 错误响应，但实现栈不同。这些数据可用于比较当前端点整体，未通过 profiling 分离解释器、请求对象、装饰器或序列化各自的成本。

### 5.2 数据库路径：小数据集下的整体差异

`getall` 包含取连接、`LEFT JOIN` 查询、行映射和 JSON 序列化，当前单核吞吐差距为 9.8 倍。数据集每库仅 2 行，因此结果主要反映该小查询与应用处理的组合，不代表大表查询也会获得同样倍数。

两端响应也不完全相同：Rust 的 `to_dict()` 不返回 `identity` / `user_identity`。驱动、数据映射和响应大小都会影响结果；本记录没有逐项消除这些变量。

### 5.3 登录慢：安全成本的体现，不是框架退步

| | SanicBoot | Stemson |
|---|---|---|
| 客户端 | 先做 MD5，再发送摘要 | 发送明文（需 TLS） |
| 服务端 | 字符串比较 | **Argon2id**（m=19 MiB, t=2, p=1） |
| 单次成本 | ~微秒 | ~16 ms CPU + 19 MiB 内存带宽 |

单核 63 RPS ≈ 每核 16 ms/次，正好是 Argon2id 的代价；8 核只到 177 RPS ≈ 2.8 核
等效。Argon2 存在内存带宽成本，但原记录未单独定位扩展瓶颈。原版 MD5 方案的问题是：
使用固定盐、计算成本过低，客户端摘要也可充当可重放凭据；当前 Rust 版不兼容该方案。

若登录吞吐成为瓶颈，先定位服务端哈希、数据库和客户端计算的占比。OPAQUE 可把 Argon2 工作放到客户端，但增加协议交互与长期密钥管理；第 8 节单独讨论它，不能据此直接推算用户的登录耗时。

### 5.4 尾延迟

除登录外，Stemson 的 p99 全面更低（0.96–5.44 ms vs 2.59–83.37 ms）。
Sanic 的 `getall` p99 达到 62–83 ms；Rust 版为 3.6–5.4 ms。这是在指定并发下观察到的结果，未单独量化排队时间。

## 6. 复现

以下是原测试的 Linux 环境准备示例，需使用不同终端分别运行 Python 服务、Rust 服务和客户端。`taskset` 与容器内网 IP 方式针对原 Linux 环境；在 macOS / Docker Desktop 上需调整连接地址和 CPU 控制方法。

先按第 2–3 节对齐数据量、连接池、日志和 CPU 资源。`.bench/stemson.yaml` 需要自行从开发配置复制，设置数据库连接和监听端口，例如 18001。Python 启动命令放在子 shell 中，其余命令仍在仓库根目录执行。

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
(cd legacy/python && PYTHONPATH=. ../../.bench/venv/bin/python backend/app.py)

# 3) Stemson
STEMSON_CONFIG=.bench/stemson.yaml cargo run --release

# 4) 压测（单核示例：服务端绑 CPU0，客户端用其余核）
cargo build --release --example loadgen
taskset -c 1-31 ./target/release/examples/loadgen \
  --url http://127.0.0.1:18001/nope --concurrency 32 --duration 8 --warmup 2
```

日常单服务压测可以使用 [`scripts/benchmark.sh`](../scripts/benchmark.sh)：

```bash
TOKEN='<有效 JWT>' \
LOGIN_BODY='{"username":"admin","password":"admin123"}' \
  scripts/benchmark.sh http://127.0.0.1:8080 32 8
```

未设置 `TOKEN` 或 `LOGIN_BODY` 时，脚本跳过对应场景。脚本默认预热 **3 秒**；原对比记录使用 **2 秒**。要复现第 3 节口径，直接调用 `loadgen` 并显式传入 `--warmup 2`，同时重复三轮取中位数。Python 原版的无鉴权 `/getall` 请求还需按第 3 节单独构造。

## 7. 局限

- 同机测试，无真实网络延迟；MySQL 与压测端共享 CPU/内存带宽。
- 数据量很小（每库 2 行），`LEFT JOIN` 成本极低；数据量增大后数据库路径的差距会变化。
- 单核/多核划分依赖本机 P-core/E-core 拓扑，其他机器绝对值会不同，倍数也只适用于这里的场景与配置。
- Sanic 的 `getall` 无鉴权，Stemson 需要 JWT：Rust 侧多了 HMAC 校验（<1 µs），影响可忽略但存在。
- 登录场景算法不同（MD5 vs Argon2id），只能说明**产品级端点**的差异，不是框架差异。
- 为了让原版在 Python 3.9 + MySQL 8.4 下跑起来，基准副本做了三处**环境性**调整（不改业务代码）：
  1. 安装 `cryptography`（PyMySQL 连 MySQL 8 的 `caching_sha2_password` 需要）；
  2. 配置里补顶层 `http.port`（原 `app.py` 读的是 `http.port` 而不是 `server.http.port`）；
  3. 关闭 `ACCESS_LOG`。

## 8. 登录：OPAQUE 与 Argon2id 的差距

OPAQUE 把 Argon2 KSF 放到客户端，服务端主要处理 OPRF、密钥交换和协议验证。因此它降低的是服务端计算负担，不代表密码派生成本消失，也不代表完整登录更快。

### 8.1 历史进程内记录

下面保留原记录的数值，**不是当前代码的重新测量结果**：

| 操作 | 原记录的平均单次成本 |
|------|----------------------|
| OPAQUE 第一轮（OPRF + 状态加密） | 229 µs |
| OPAQUE 第二轮 | 2 µs |
| 两步之和 | 231 µs |
| Argon2id 校验 | 14,788 µs |
| Argon2id 生成哈希 | 13,266 µs |

按倒数计算，`1 / 231 µs ≈ 4,329 次/秒`，`1 / 14,788 µs ≈ 68 次/秒`，比值约 64。它只是历史记录中服务端计算量的比较，**不包含 HTTP、数据库、客户端 KSF 或 JWT 签发，不能直接当作完整登录容量**。

原测量工具入口如下；当前版本无法完成整轮测量，问题见下文：

```bash
cargo run --release --example opaque_cost
```

复测时需要核对计时边界：当前 `examples/opaque_cost.rs` 的第二轮计时闭包同时调用 `login_start` 和 `login_finish`，打印的 total 又加了一次 start；因此不能按打印标签直接解释为互不重叠的两步耗时。此外，它在第二轮循环中新建服务端状态，却复用先前交换生成的客户端证明，证明与新状态不匹配。本次以 debug 构建运行该示例，确认在 `examples/opaque_cost.rs:99` 的 `expect("login finish")` 处因认证失败退出。该示例需要修正后再用于有效复测；历史数值不能据它视为已验证。

### 8.2 历史 HTTP 端点记录

原记录为单 P-core、并发 32：

| 端点 | RPS | p50 | p99 |
|------|-----|-----|-----|
| `POST /v1/api/auth/login/start` | 5,668 | 5.61 ms | 7.56 ms |
| `POST /v1/api/auth/login/finish` | 8,450 | 3.63 ms | 5.92 ms |

这两组数字复用预先构造的协议消息，不包含客户端 KSF。当前默认 `single_use: true` 会拒绝重复提交相同 session，所以第二轮数据不能直接作为当前默认配置下有效登录的吞吐。

只有假设两轮共享同一瓶颈、每轮成本稳定且消息有效时，才可粗略计算 `1 / (1/5668 + 1/8450) ≈ 3,392` 次登录/秒。原记录不满足完整流程验证的条件，应保留为端点成本参考，不作为容量承诺。

### 8.3 怎样评估真实登录体验

复测完整流程时，每次登录都应生成新的客户端状态与证明，并计入：

1. 客户端 KSF 和协议计算。
2. 两次 HTTP 往返。
3. 服务端数据库查询、账户检查与登录时间更新。
4. session 加解密、一次性标记与 JWT 签发。

单实例内存标记与多副本 Redis 标记应分别测试；它们具有不同的共享状态和网络开销。本文没有提供上述完整流程的新结果。
