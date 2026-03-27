<p align="center">
  <h1 align="center">forge</h1>
</p>

<p align="center">
  <b>工程定义协议 + 轻量运行时。一份 forge.toml，定义整个项目。</b>
</p>

<p align="center">
  <a href="https://github.com/anthropics/forge/actions"><img src="https://img.shields.io/github/actions/workflow/status/anthropics/forge/ci.yml?style=flat-square" alt="CI"></a>
  <a href="https://crates.io/crates/forge-cli"><img src="https://img.shields.io/crates/v/forge-cli?style=flat-square" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
</p>

<br>

`forge.toml` 是工程的 **single source of truth** —— 声明服务、依赖、端口、健康检查和自定义命令。`fr`（forge 运行时）让这份声明直接可执行：依赖感知地启动服务、做健康检查门控、统一入口管理生命周期。

```
$ fr up
 Level 0
+----------------------+  +----------------------+
| + postgres           |  | + redis              |
| :5432  healthy       |  | :6379  healthy       |
+----------------------+  +----------------------+
            |
 Level 1
+----------------------+  +----------------------+
| + api/server         |  | + worker             |
| :8080  healthy       |  | :9000  healthy       |
+----------------------+  +----------------------+
            |
 Level 2
+----------------------+
| + web/app            |
| :3000  healthy       |
+----------------------+

  OK 5/5 services started

┏━━━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━┳━━━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━┳━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ SERVICE              ┃ PORT   ┃ HEALTH    ┃ PID     ┃ RESTART ┃ TIME   ┃ DEPENDS ON               ┃
┡━━━━━━━━━━━━━━━━━━━━━━╇━━━━━━━━╇━━━━━━━━━━━╇━━━━━━━━━╇━━━━━━━━━╇━━━━━━━━╇━━━━━━━━━━━━━━━━━━━━━━━━━━┩
│ postgres             │ 5432   │ healthy   │ 12301   │ 0       │ 1.2s   │ -                        │
│ redis                │ 6379   │ healthy   │ 12302   │ 0       │ 0.8s   │ -                        │
│ api/server           │ 8080   │ healthy   │ 12345   │ 0       │ 3.1s   │ postgres, redis          │
│ worker               │ 9000   │ healthy   │ 12346   │ 0       │ 2.4s   │ postgres, redis          │
│ web/app              │ 3000   │ healthy   │ 12389   │ 0       │ 4.7s   │ api/server               │
└──────────────────────┴────────┴───────────┴─────────┴─────────┴────────┴──────────────────────────┘
```

---

## 为什么需要 forge

多服务项目有一些几乎必然出现的问题，而现有工具往往是在各自的职责边界内解决自己那一块，但项目真正的复杂度恰恰在这些边界之间。

---

**服务之间有依赖，但启动不知道这件事。**

数据库要先就绪，依赖它的服务才能起来。这个顺序写在 README 里，靠人记、靠人执行。新人第一次跑项目，大概率要调试半小时才弄清楚该按什么顺序来。"等数据库就绪"更棘手——不是进程起了就算就绪，而是真正能接受连接了才算，这个判断通常靠手感或者 sleep 几秒。

进程管理工具负责启动进程，但大多不理解服务之间的依赖关系，或者有基础的依赖声明但没有健康检查门控——依赖方进程启动了，但不等它真正健康就继续启动下游，结果下游启动失败、报连接错误，排查起来费时费力。

forge 的处理：`depends_on` 声明依赖，健康检查（HTTP 探测或命令探测）作为门控，上游真正健康后才启动下游。这个逻辑写在配置里，不依赖人的记忆，也不依赖 sleep。

---

**没有人真正知道项目的完整拓扑。**

一个项目有哪些服务、各自监听什么端口、谁依赖谁——这些信息分散在进程管理配置、基础设施配置、各服务的启动脚本里，格式各异。想获得整体视图，需要把所有配置文件读完再自己在脑子里拼出来。

这个问题在服务少的时候不明显，服务多了之后就成了真实的认知负担：新人上手、排查问题、做跨服务的改动，都需要先花时间还原这个全局视图。

forge 的处理：`fr inspect` 直接输出完整的工程拓扑——所有服务、端口、依赖关系、健康状态，一次调用，人类可读，也可以加 `--json` 给程序消费。这个视图不是手写维护的，它从 `forge.toml` 实时推导，永远准确。

---

**加一个服务，要改的地方太多。**

进程管理配置要加这个服务，构建配置可能也要加，如果有任务编排工具还要在那里注册，最后别忘了更新 README 里的"如何启动"说明。漏了哪一步，别人就跑不起来，或者功能运行了但某个环节没被覆盖到。

这种散装的维护方式还有个隐患：配置与文档之间很容易出现漂移。服务改了端口，进程管理配置里改了，但 README 里的端口说明忘了改；半年后有人对着文档操作，怎么都对不上。

forge 的处理：每个服务只需要在自己目录里放一个 `forge.toml`，声明它自己的信息。forge 运行时递归扫描自动发现，不需要在任何中心配置里注册。`forge.toml` 就是这个服务的文档，也是它的运行配置，两者是同一份东西，不存在漂移。

---

**操作入口分散，没有统一的方式管理项目。**

启动这个服务用一个命令，启动那个用另一个，基础设施又是另一套工具。想做跨服务的操作——比如按顺序跑所有服务的数据库迁移——没有统一入口，只能自己写脚本或手动逐个执行，还要自己保证顺序正确。

forge 的处理：`fr up / down / restart / logs / ps / run <cmd>` 是统一的操作入口，适用于所有服务，不论它们用什么语言、什么构建工具。`fr run migrate` 会按拓扑序在每个声明了 migrate 命令的服务里执行，顺序由依赖关系决定，不需要人来排。

---

**forge 的核心是协议（forge.toml），运行时（fr）只是让协议可执行。** 把"项目长什么样"和"怎么操作项目"合并在一份声明里，解决的是这些问题在根源处的共同原因：工程信息没有一个统一的、可执行的表达形式。

---

## 特性

- **依赖感知启动** —— 基于 `depends_on` 做拓扑排序，上游健康后才启动下游
- **健康检查门控** —— HTTP 探测（`health.http = "/healthz"`）或命令探测（`health.cmd = "pg_isready"`）
- **分布式配置** —— 每个服务拥有自己的 `forge.toml`，新增服务不需要修改根配置
- **域级操作** —— `fr up api` 同时启动 `api/server` + `api/worker` 及全部传递依赖
- **混合前后台** —— `fr up --attach api/server` 只前台指定服务，其余后台；支持 toml 配置默认行为
- **自定义指令** —— `fr run migrate` 按拓扑序在各服务中执行自定义命令
- **崩溃自动恢复** —— `autorestart`、`max_restarts`、`restart_delay` 可配
- **端口冲突检测** —— 启动前预检，服务间重复和系统占用一网打尽
- **工程查询** —— `fr inspect --json` 输出完整工程拓扑，机器可读也人类可读
- **结构化输出** —— 所有命令支持 `--json`
- **跨平台** —— macOS / Linux / Windows，单个静态二进制（~6 MB），零运行时依赖
- **多语言** —— 内置 `rust`、`node`、`go`、`java`、`command` 类型，任何语言通过自定义命令接入

---

## 安装

### 一键安装（推荐）

从 GitHub Releases 下载预编译二进制，无需 Rust：

```bash
curl -fsSL https://raw.githubusercontent.com/liuxin231/forge/main/install.sh | bash
```

支持平台：macOS（Intel / Apple Silicon）、Linux（x86_64 / aarch64）

安装位置：`~/.forge/bin/fr`，脚本自动配置 PATH。

### 升级

```bash
fr upgrade          # 升级到最新版本
fr upgrade --check  # 仅检查是否有新版本，不安装
```

### 从源码构建（需要 Rust 1.82+）

```bash
git clone https://github.com/liuxin231/forge.git
cd forge
cargo install --path .
```

### 手动构建

```bash
cargo build --release
cp target/release/fr ~/.local/bin/
```

### 验证

```bash
fr --version
```

---

## 快速开始

### 1. 创建根配置

在项目根目录创建 `forge.toml`：

```toml
[workspace]
name = "my-project"

[workspace.zones]
apps = "apps"
infra = "infra"
```

### 2. 定义服务

每个服务目录下放一个 `forge.toml`：

```toml
# apps/api/server/forge.toml
[service]
type = "rust"
port = 8080
depends_on = ["postgres", "redis"]
health.http = "/healthz"

[service.env]
RUST_LOG = "info"
DATABASE_URL = "postgres://localhost:5432/myapp"

[service.commands.migrate]
run = "sqlx migrate run"
```

基础设施服务可以共享一个文件：

```toml
# infra/forge.toml
[service.postgres]
type = "command"
port = 5432
up = "docker compose up -d postgres"
down = "docker compose down postgres"
health.cmd = "docker compose exec postgres pg_isready"

[service.redis]
type = "command"
port = 6379
up = "docker compose up -d redis"
down = "docker compose down redis"
health.cmd = "docker compose exec redis redis-cli ping"
```

### 3. 运行

```bash
fr up                              # 全部启动（后台，按依赖拓扑排序）
fr up --attach                     # 全部启动（前台，Ctrl+C 停止）
fr up --attach api/server          # 仅 api/server 前台，其余后台
fr up api                          # 启动 api 域 + 所有依赖
fr ps                              # 查看状态
fr ps --json                       # JSON 格式状态
fr inspect                         # 查看工程拓扑（人类可读）
fr inspect --json                  # 查看工程拓扑（机器可读）
fr logs api/server                 # 流式查看日志
fr run migrate                     # 按拓扑序执行各服务的 migrate 命令
fr restart api/server              # 重启单个服务
fr down                            # 停止全部
```

---

## 输出样例

以下是一个典型的全栈项目（Next.js 前端 + Rust API 后端 + PostgreSQL + Redis）的实际输出。

### 工程结构

```
shopify-clone/
├── forge.toml                        # 根配置
├── apps/
│   ├── api/
│   │   └── server/
│   │       ├── forge.toml            # type = "rust", port = 8080
│   │       ├── Cargo.toml
│   │       └── src/
│   ├── worker/
│   │   ├── forge.toml                # type = "rust", port = 9000
│   │   ├── Cargo.toml
│   │   └── src/
│   └── web/
│       └── app/
│           ├── forge.toml            # type = "node", port = 3000
│           ├── package.json
│           └── src/
├── infra/
│   ├── forge.toml                    # postgres, redis
│   └── docker-compose.yml
└── libs/
    └── shared/                       # 共享库（无 forge.toml，自动忽略）
```

**根 `forge.toml`：**

```toml
[workspace]
name = "shopify-clone"
description = "电商平台全栈工程"

[workspace.zones]
apps = "apps"
infra = "infra"

[commands.setup]
description = "初始化开发环境（启动基础设施）"
mode = "direct"
run = "docker compose -f infra/docker-compose.yml up -d"

[commands.migrate]
description = "运行数据库迁移"
mode = "service"
order = "topological"
fail_fast = true

[commands.seed]
description = "填充测试数据"
mode = "service"
order = "topological"
```

**`apps/api/server/forge.toml`：**

```toml
[service]
type = "rust"
port = 8080
depends_on = ["postgres", "redis"]
health.http = "/healthz"
attach = true

[service.env]
RUST_LOG = "info"
DATABASE_URL = "postgres://postgres:password@localhost:5432/shopify"
REDIS_URL = "redis://localhost:6379"
JWT_SECRET = "dev-secret"

[service.commands.migrate]
run = "sqlx migrate run --database-url $DATABASE_URL"

[service.commands.seed]
run = "cargo run --bin seed"

[service.commands.lint]
run = "cargo clippy -- -D warnings"
```

**`apps/worker/forge.toml`：**

```toml
[service]
type = "rust"
port = 9000
depends_on = ["postgres", "redis"]
health.http = "/health"

[service.env]
RUST_LOG = "info"
DATABASE_URL = "postgres://postgres:password@localhost:5432/shopify"
REDIS_URL = "redis://localhost:6379"
QUEUE_CONCURRENCY = "4"
```

**`apps/web/app/forge.toml`：**

```toml
[service]
type = "node"
port = 3000
depends_on = ["api/server"]
health.http = "/"

[service.env]
NEXT_PUBLIC_API_URL = "http://localhost:8080"
NODE_ENV = "development"

[service.commands.lint]
run = "next lint"
```

**`infra/forge.toml`：**

```toml
[service.postgres]
type = "command"
port = 5432
up = "docker compose -f docker-compose.yml up -d postgres"
down = "docker compose -f docker-compose.yml stop postgres"
health.cmd = "docker compose -f docker-compose.yml exec -T postgres pg_isready -U postgres"

[service.redis]
type = "command"
port = 6379
up = "docker compose -f docker-compose.yml up -d redis"
down = "docker compose -f docker-compose.yml stop redis"
health.cmd = "docker compose -f docker-compose.yml exec -T redis redis-cli ping"
```

---

### `fr up` — 启动全部服务

```
$ fr up

 Level 0
+----------------------+  +----------------------+
| . postgres           |  | . redis              |
|        pending       |  |        pending       |
+----------------------+  +----------------------+

 Level 1
+----------------------+  +----------------------+
| . api/server         |  | . worker             |
|        pending       |  |        pending       |
+----------------------+  +----------------------+
            |
 Level 2
+----------------------+
| . web/app            |
|        pending       |
+----------------------+

 Level 0
+----------------------+  +----------------------+
| + postgres           |  | + redis              |
| :5432  healthy       |  | :6379  healthy       |
+----------------------+  +----------------------+
            |
 Level 1
+----------------------+  +----------------------+
| + api/server         |  | + worker             |
| :8080  healthy       |  | :9000  healthy       |
+----------------------+  +----------------------+
            |
 Level 2
+----------------------+
| + web/app            |
| :3000  healthy       |
+----------------------+

  OK 5/5 services started

┏━━━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━┳━━━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━┳━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ SERVICE              ┃ PORT   ┃ HEALTH    ┃ PID     ┃ RESTART ┃ TIME   ┃ DEPENDS ON               ┃
┡━━━━━━━━━━━━━━━━━━━━━━╇━━━━━━━━╇━━━━━━━━━━━╇━━━━━━━━━╇━━━━━━━━━╇━━━━━━━━╇━━━━━━━━━━━━━━━━━━━━━━━━━━┩
│ postgres             │ 5432   │ healthy   │ 12301   │ 0       │ 1.2s   │ -                        │
│ redis                │ 6379   │ healthy   │ 12302   │ 0       │ 0.8s   │ -                        │
│ api/server           │ 8080   │ healthy   │ 12345   │ 0       │ 3.1s   │ postgres, redis          │
│ worker               │ 9000   │ healthy   │ 12346   │ 0       │ 2.4s   │ postgres, redis          │
│ web/app              │ 3000   │ healthy   │ 12389   │ 0       │ 4.7s   │ api/server               │
└──────────────────────┴────────┴───────────┴─────────┴─────────┴────────┴──────────────────────────┘
```

---

### `fr ps` — 查看服务状态

```
$ fr ps

┏━━━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━━┳━━━━━━━━━━━━┳━━━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━━┓
┃ SERVICE              ┃ PORT     ┃ STATUS     ┃ HEALTH    ┃ PID     ┃ RESTART ┃
┡━━━━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━━╇━━━━━━━━━━━━╇━━━━━━━━━━━╇━━━━━━━━━╇━━━━━━━━━┩
│ postgres             │ 5432     │ running    │ healthy   │ 12301   │ 0       │
│ redis                │ 6379     │ running    │ healthy   │ 12302   │ 0       │
│ api/server           │ 8080     │ running    │ healthy   │ 12345   │ 0       │
│ worker               │ 9000     │ running    │ healthy   │ 12346   │ 0       │
│ web/app              │ 3000     │ running    │ healthy   │ 12389   │ 0       │
└──────────────────────┴──────────┴────────────┴───────────┴─────────┴─────────┘
```

---

### `fr inspect --json` — 工程拓扑（机器可读）

```
$ fr inspect --json

{
  "name": "shopify-clone",
  "root": "/home/user/shopify-clone",
  "services": [
    {
      "name": "postgres",
      "type": "command",
      "port": 5432,
      "depends_on": [],
      "health": { "cmd": "docker compose ... pg_isready" },
      "status": "running"
    },
    {
      "name": "redis",
      "type": "command",
      "port": 6379,
      "depends_on": [],
      "health": { "cmd": "docker compose ... redis-cli ping" },
      "status": "running"
    },
    {
      "name": "api/server",
      "type": "rust",
      "port": 8080,
      "depends_on": ["postgres", "redis"],
      "health": { "http": "/healthz" },
      "status": "running"
    },
    {
      "name": "worker",
      "type": "rust",
      "port": 9000,
      "depends_on": ["postgres", "redis"],
      "health": { "http": "/health" },
      "status": "running"
    },
    {
      "name": "web/app",
      "type": "node",
      "port": 3000,
      "depends_on": ["api/server"],
      "health": { "http": "/" },
      "status": "running"
    }
  ]
}
```

---

### `fr run migrate` — 按拓扑序执行迁移

```
$ fr run migrate

  Running: api/server → sqlx migrate run --database-url $DATABASE_URL
  Applied 3 migrations (api/server)

  worker has no migrate command, skipping
  web/app has no migrate command, skipping

  OK migrate completed (1/1 services)
```

---

### `fr up api` — 域级启动（含传递依赖）

```
$ fr up api

Resolved targets: api/server, worker
With dependencies: postgres, redis, api/server, worker

 Level 0
+----------------------+  +----------------------+
| + postgres           |  | + redis              |
| :5432  healthy       |  | :6379  healthy       |
+----------------------+  +----------------------+
            |
 Level 1
+----------------------+  +----------------------+
| + api/server         |  | + worker             |
| :8080  healthy       |  | :9000  healthy       |
+----------------------+  +----------------------+

  OK 4/4 services started
```

---

### `fr down` — 停止全部服务

```
$ fr down

  Stopping web/app...     done
  Stopping api/server...  done
  Stopping worker...      done
  Stopping redis...       done
  Stopping postgres...    done

  OK 5/5 services stopped
```

---

## 命令

| 命令 | 说明 |
|------|------|
| `fr up [targets...] [--attach [svc...]] [--json]` | 按依赖拓扑序启动服务 |
| `fr down [targets...] [--json]` | 按逆序停止服务 |
| `fr restart [targets...] [--json]` | 重启服务 |
| `fr ps [targets...] [--json]` | 查看服务状态 |
| `fr logs [targets...] [-n N] [-f] [--json]` | 查看 / 流式日志 |
| `fr inspect [target] [--json]` | 查询工程拓扑或单个服务详情 |
| `fr run <name> [targets...] [--parallel] [--json]` | 执行自定义指令 |
| `fr graph [targets...]` | 输出依赖关系图 |
| `fr init [path]` | 初始化新工作区 |

### 目标解析

| 输入 | 解析为 |
|------|--------|
| *（不指定）* | 所有 `role = "service"` 的服务 |
| `api` | `api/` 下所有服务 —— `api/server`、`api/worker`、... |
| `api/server` | 精确匹配 |
| `postgres` | 精确匹配（infra 服务） |

依赖始终自动包含。`fr up web/app` 会同时拉起依赖链上的 `api/server`、`postgres`、`redis`。

### 前后台模式

| 用法 | 行为 |
|------|------|
| `fr up` | 全部后台（supervisor） |
| `fr up --attach` | 按 toml 中 `attach = true` 的服务前台，其余后台；无配置则全部前台 |
| `fr up --attach api/server web/app` | 仅指定服务前台，其余后台 |

Ctrl+C 只停前台服务，后台服务继续运行。

---

## 配置

### 根 `forge.toml`

```toml
[workspace]
name = "my-project"
description = "可选描述"

[workspace.zones]                  # 可选，不配则从项目根目录递归扫描
apps = "apps"
infra = "infra"

# [workspace.ignore]              # 可选，追加到内置默认忽略列表
# patterns = ["tmp", "build-*"]

# 自定义指令（工程级）
[commands.setup]
description = "初始化开发环境"
mode = "direct"                    # 在工程根目录直接执行
run = "docker compose -f infra/docker-compose.yml up -d"

[commands.migrate]
description = "运行数据库迁移"
mode = "service"                   # 委派给各服务执行
order = "topological"              # topological | parallel | sequential
```

<details>
<summary><b>内置默认忽略目录</b></summary>

`node_modules`、`target`、`dist`、`.git`、`.next`、`.nuxt`、`.output`、`__pycache__`、`vendor`、`.turbo`、`.nx`、`.forge`

</details>

### 服务 `forge.toml`

```toml
[service]
type = "rust"                      # "rust" | "node" | "go" | "java" | "command"
role = "service"                   # "service"（默认）| "cli"
port = 8080
depends_on = ["postgres", "redis"]
groups = ["backend", "core"]
attach = true                      # fr up --attach 时默认前台运行此服务

health.http = "/healthz"           # HTTP 探测 —— 或：
# health.cmd = "curl -sf ..."      # 命令探测（exit 0 = 健康）

[service.env]
RUST_LOG = "info"

# 自定义指令（服务级）
[service.commands.migrate]
run = "sqlx migrate run"

[service.commands.lint]
run = "cargo clippy"
```

<details>
<summary><b>完整字段参考</b></summary>

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | string | `"command"` | 服务类型：`rust`、`node`、`go`、`java`、`command` |
| `role` | string | `"service"` | `"service"`（常驻进程）或 `"cli"`（仅构建，`fr up` 不启动） |
| `port` | u16 | — | 声明端口，用于健康检查、冲突检测、状态展示 |
| `depends_on` | string[] | `[]` | 依赖的服务列表，必须健康后才启动当前服务 |
| `groups` | string[] | `[]` | 标签组，用于分组操作 |
| `attach` | bool | `false` | `fr up --attach` 时是否默认前台运行 |
| `health.http` | string | — | HTTP 健康检查路径 |
| `health.cmd` | string | — | 命令健康检查（exit 0 = 健康） |
| `env` | table | `{}` | 启动时注入的环境变量 |
| `up` | string | — | 覆盖启动命令 |
| `down` | string | — | 覆盖停止命令 |
| `dev` | string | — | 开发模式启动命令 |
| `build` | string | — | 构建命令 |
| `cwd` | string | forge.toml 所在目录 | 工作目录 |
| `args` | string | — | 传递给服务进程的参数 |
| `autorestart` | bool | `true` | 崩溃后自动重启 |
| `max_restarts` | u32 | `10` | 最大连续重启次数 |
| `restart_delay` | u64 | `3` | 重启间隔（秒） |
| `kill_timeout` | u64 | `10` | SIGTERM 超时后发 SIGKILL（秒） |
| `treekill` | bool | `true` | 停止时杀掉整个进程树 |
| `commands.*` | table | — | 自定义指令，通过 `fr run <name>` 调用 |

</details>

### 自定义指令

两种模式：

**直接执行** —— 在工程根目录运行一个命令：

```toml
# 根 forge.toml
[commands.setup]
mode = "direct"
run = "docker compose -f infra/docker-compose.yml up -d"
```

**服务委派** —— 按拓扑序遍历各服务，执行服务自己定义的同名命令：

```toml
# 根 forge.toml
[commands.migrate]
mode = "service"
order = "topological"              # topological | parallel | sequential
fail_fast = true                   # 一个失败是否终止

# 各服务的 forge.toml
[service.commands.migrate]
run = "sqlx migrate run"
```

---

## 工作原理

### 服务发现

```
fr up
 │
 ├─ 读取根 forge.toml → 获取 [workspace.zones]
 ├─ 递归扫描每个 zone 目录下的 forge.toml
 ├─ 跳过匹配忽略规则的目录
 ├─ 解析 [service]（单服务）或 [service.*]（多服务）
 └─ 构建包含所有已发现服务的 ProjectConfig
```

服务名由目录路径相对于 zone 根目录自动推导：

```
apps/api/server/forge.toml           →  api/server
apps/web/app/forge.toml              →  web/app
infra/forge.toml [service.postgres]  →  postgres
```

### 启动流程

```
fr up web/app
 │
 ├─ 解析目标：web/app
 ├─ 展开依赖：web/app → api/server → postgres, redis
 ├─ 拓扑排序：postgres → redis → api/server → web/app
 ├─ 端口冲突检查
 └─ 按顺序逐个启动：
      ├─ 启动进程
      ├─ 等待健康检查通过 ✓
      └─ 继续下一个
```

### 运行时状态

```
.forge/                           # 加入 .gitignore
├── supervisor.pid
├── supervisor.port
├── pids/
│   ├── api-server.pid
│   └── postgres.pid
└── logs/
    ├── api-server/out.log
    └── postgres/out.log
```

supervisor 是**项目级**的 —— 每个项目独立一个。`fr up` 时启动，`fr down` 所有服务停止后自动退出。没有全局守护进程。

---

## forge 不做什么

- **不是构建系统** —— 不替代 `cargo build` / `pnpm build`，只负责在正确的时机调用它们
- **不是容器运行时** —— 不替代 Docker，只负责编排 `docker compose`
- **不是 CI/CD** —— 不做流水线、不做部署

---

## 参与贡献

```bash
git clone https://github.com/anthropics/forge.git
cd forge
cargo build
cargo test
```

## License

[MIT](LICENSE)
