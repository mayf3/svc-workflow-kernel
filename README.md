# svc-workflow

`svc-workflow` 是 Rust + PostgreSQL 实现的串行受治理工作流内核。PostgreSQL
保存权威事实，Context Revision、Node Visit、Submission 与 Event 均不可变，
Instance 仅保存可重建的当前投影。

## 当前状态

冻结领域版本：`v0.3.1`。

| 切片 | 能力 | 状态 |
|---|---|---|
| PR 1 | PostgreSQL Storage Foundation | MERGED |
| PR 2 | Definition Version Service | MERGED |
| PR 3A–3D | Create / Revise / Transition / Atomic Combined Command | MERGED |
| PR 4 | Read Model / Query Service | MERGED |
| PR 5 | Admin Emergency Commands / Projection Repair | MERGED |
| PR 6A | Legacy ADC Initial Import | MERGED，`LOCAL_IMPORT_READY` |
| PR 6B+ | Relay / Shadow / Cutover | BLOCKED，`SHADOW_NOT_READY`、`CUTOVER_NOT_READY` |

PR 6B+ 仍受三个 Legacy 边界阻断：ADC 启动时覆盖模板、非标准 `currentStep`
写路径绕过 Relay、Domain Owner 不唯一。它们不影响本地导入原语，但必须在 Shadow 或
Cutover 前关闭，具体边界见 Legacy ADC 迁移文档。

当前尚未提供 HTTP/gRPC 服务；可用入口是 Rust application service。

## 文档入口

主线只保留当前有效合同，不保存已结束 PR 的长篇审计快照；历史审计仍可从 Git
对应提交恢复。

| 类型 | 文档 |
|---|---|
| 冻结领域架构 | [SVC_WORKFLOW_ARCHITECTURE_V0_3_1.md](docs/architecture/SVC_WORKFLOW_ARCHITECTURE_V0_3_1.md) |
| 实施层总契约 | [IMPLEMENTATION_CONTRACT_V0_1.md](docs/contracts/IMPLEMENTATION_CONTRACT_V0_1.md) |
| 存储合同 | [POSTGRES_STORAGE_CONTRACT_V0_1.md](docs/contracts/POSTGRES_STORAGE_CONTRACT_V0_1.md) |
| Definition Service 合同 | [DEFINITION_SERVICE_CONTRACT_V0_1.md](docs/contracts/DEFINITION_SERVICE_CONTRACT_V0_1.md) |
| Runtime 创建合同 | [WORKFLOW_INSTANCE_CREATE_CONTRACT_V0_1.md](docs/contracts/WORKFLOW_INSTANCE_CREATE_CONTRACT_V0_1.md) |
| Runtime 流转合同（含 PR 3D） | [WORKFLOW_TRANSITION_CONTRACT_V0_1.md](docs/contracts/WORKFLOW_TRANSITION_CONTRACT_V0_1.md) |
| Runtime 查询合同 | [WORKFLOW_QUERY_CONTRACT_V0_1.md](docs/contracts/WORKFLOW_QUERY_CONTRACT_V0_1.md) |
| Admin Recovery 合同 | [ADMIN_RECOVERY_CONTRACT_V0_1.md](docs/contracts/ADMIN_RECOVERY_CONTRACT_V0_1.md) |
| Legacy Initial Import 合同 | [LEGACY_IMPORT_CONTRACT_V0_1.md](docs/contracts/LEGACY_IMPORT_CONTRACT_V0_1.md) |
| Legacy ADC 迁移 | [LEGACY_ADC_MIGRATION_V0_1.md](docs/migration/LEGACY_ADC_MIGRATION_V0_1.md) |

## 维护规则

1. 每个切片基于最新 `main`，独立审计关闭全部 Blocker/High 后只做
   `git merge --ff-only`。
2. PostgreSQL 是唯一权威状态源；Instance 当前字段只是可重建投影。
3. 一次成功状态命令只增加一个状态版本，并只写一条对应 Event。
4. 当前树只保留冻结架构和仍有效合同；已结束 PR 的调查与审计证据保留在 Git 历史。

## 本地验证

测试默认连接：

```text
postgres://postgres:postgres@localhost:5432/svc_workflow
```

启动 PostgreSQL 后运行：

```bash
docker compose up -d postgres
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
