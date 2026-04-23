# DAO Treasury MVP 开发计划

这个目录是 DAO treasury / governance MVP 的本地实验区。原则是先把投票、snapshot、tally、钱包交互这些流程跑通，再进入 CKB consensus 层的 treasury bucket 改造。

## 目录约定

```text
dao-treasury/
  mvp.md                 # 当前 MVP 计划和环境记录
  ckb.toml               # 本地 dev chain 节点配置，由 ckb init 生成
  ckb-miner.toml         # 本地 miner 配置，由 ckb init 生成
  specs/                 # 本地 dev chain spec
  data/                  # 本地 CKB DB，git ignored
  accounts/              # 本地 dev 私钥 / keystore，git ignored
  artifacts/             # proposal / snapshot / vote / tally 产物，git ignored
  scripts/               # 本实验的辅助脚本
  tally-service/          # Rust Tally JSON-RPC service MVP
```

## MVP 目标

第一版 MVP 不修改 CKB consensus，不真的释放 treasury bucket，而是先做一个可以端到端验证的治理投票原型：

1. 启动一个独立的 CKB dev chain，DB 放在 `dao-treasury/data/`。
2. 创建若干本地账户：faucet/miner、Alice、Bob、Carol、Proposer、Treasury Recipient。
3. 给 Alice/Bob/Carol 转入 CKB，并把一部分资金存入 Nervos DAO。
4. 创建一个 proposal，内容使用链上 commitment + 链下 manifest 的方式表达。
5. 在某个 snapshot block 固定 eligible DAO deposits。
6. 生成 snapshot 文件和 Merkle root，并把 root 作为链上 commitment 的候选格式。
7. 让 Alice/Bob/Carol 对 proposal 投票。
8. Rust Tally service 根据 snapshot 和链上 vote cells 提供 proposal、voter options、proof、tally 查询。
9. 独立 verifier 可以重新扫描链、重新生成 snapshot、重新计算 tally，确认结果一致。

## 本阶段不做

- 不做代理投票 / delegation。
- 不要求 CKB node 在 consensus 中生成 snapshot。
- 不把完整 proposal 正文塞进一个交易。
- 不做真实 treasury bucket 的 consensus 释放逻辑。
- 不做主网兼容迁移，只做本地 dev chain 和测试工具。

## 本地链设计

本地链用 CKB dev chain。`ckb -C dao-treasury` 会让配置、spec、DB 都留在这个目录下。

推荐本地 RPC：

```bash
http://127.0.0.1:8114
```

本地调试可以打开 `Indexer` 和 `IntegrationTest` RPC 模块。`IntegrationTest` 只用于本地快速出块，后续接近真实环境时再切到 miner 出块。

## 账户设计

| 账户 | 用途 |
| --- | --- |
| faucet/miner | dev chain 初始资金和出块奖励 |
| Alice | DAO depositor / voter |
| Bob | DAO depositor / voter |
| Carol | DAO depositor / voter |
| Proposer | 创建 proposal |
| Treasury Recipient | 模拟 treasury 释放目标地址 |

第一版可以用 throwaway 私钥或 dev chain 内置私钥。任何真实私钥都不能放进这个目录。

## Proposal 数据模型

proposal 分两层，并引用某一次投票周期 snapshot：

1. 链上 Proposal Session Cell 存最小元数据和 commitment。
2. 链下 manifest 存完整正文、参数、讨论链接、版本等内容。

Proposal Session Cell 由 governance type script 识别。链上 data 至少包含：

```text
magic: "CKB_GOV_PROPOSAL_V1"
proposal_id
manifest_hash
manifest_uri
snapshot_id
snapshot_root
snapshot_block
snapshot_block_hash
vote_start_block
vote_end_block
choices
```

manifest 文件放入 `artifacts/`，可以进一步发布到 IPFS/Arweave/GitHub release。投票者和 verifier 用 `manifest_hash` 确认内容没有被修改。

## Governance Type Script

当前 MVP 使用标准 type script 模型。链上 governance objects 由同一个 governance type script 识别，`args` 的第一个 byte 表示对象类型：

```text
0x00 || proposal_id
  Proposal Session Cell

0x01 || proposal_id || deposit_out_point_key_hash
  Vote Cell
```

这样钱包和 indexer 不需要靠 `output_data` 前缀猜 cell 类型：

1. 发现 proposal：按 `script_type = type`、governance type code hash、args prefix `0x00` 查询。
2. 发现某个 proposal 的 votes：按 governance type code hash、args prefix `0x01 || proposal_id` 查询。
3. 做 final tally 时仍扫描完整 vote window 的历史区块输出，过滤条件是 output type script。
4. data 只承载 canonical commitment，不再作为 discovery key。

本地 MVP 可以先用 always-success 作为 governance type script 占位合约；后续替换成真实合约时，type script 应该验证 data commitment、proposal/vote args、时间窗口、proof witness 等约束。

脚本里 governance type code hash 通过环境变量配置：

```text
DAO_TREASURY_GOV_TYPE_CODE_HASH
DAO_TREASURY_GOV_TYPE_HASH_TYPE
```

未配置时，脚本会使用一个本地 placeholder code hash，只用于生成 artifact 和说明 args 结构；真正上链时必须换成已部署 governance type script 的 code hash，并把该 code cell 放进 transaction cell deps。

## Snapshot 规则

第一版主线 snapshot 是投票周期级别的数据，不属于单个 proposal。它只统计 snapshot block 时仍处于 deposit phase 的 Nervos DAO cells。同一轮 snapshot 可以被多个 proposal 引用。

当前 MVP 已切到 proof-first snapshot v3：主 snapshot 文件不再携带完整 `records` 数组，只携带 metadata 和 roots。完整 records、owner index、membership proofs 是 provider-side artifacts，钱包和 tally 通过 proof 验证，不再下载和线性扫描巨大 snapshot。

每条 eligible record 至少包含：

```text
deposit_out_point
lock_script_hash
deposit_capacity
deposit_block_number
weight
```

暂定第一版 `weight = deposit_capacity`。是否使用按 DAO 时间加权或 normalized capacity，留到下一轮讨论。

snapshot 由独立程序生成，不放进 CKB node consensus。任何人都可以运行同一个程序，基于本地 full node / indexer 重算 `record_map_root`、`owner_index_root` 和 `snapshot_root`。

snapshot 文件不包含 `proposal_id`。proposal manifest 后续通过 `snapshot_id` / `snapshot_root` 引用某一份 snapshot。

## 投票规则

第一版投票不锁 DAO deposit 本身，避免破坏用户 DAO 存款状态。

投票交易需要证明 voter 控制 snapshot 中 DAO deposit 的 lock：

1. vote cell 中引用 `proposal_id`、`deposit_out_point`、choice。
2. vote tx 至少花费一个 input，其 lock script 等于 snapshot record 中的 DAO deposit lock script。
3. vote cell 携带该 `deposit_out_point` 的 record membership proof，证明它属于 proposal 引用的 `snapshot_root`。
4. tally 只接受 proof 有效、proposal id 匹配、ownership proof 有效的 vote。
5. 同一个 `deposit_out_point` 多次投票时，默认以后出现的有效 vote 为准，顺序按 `(block_number, tx_index, output_index)`。

如果用户在 snapshot 后取出 DAO deposit，已经投出的 vote 对该 proposal 仍然有效。因为权重固定在 snapshot block，而不是投票结束时重新计算。

## Tally 和验证

MVP 主线切到 Rust Tally service。它是独立进程，不参与 CKB consensus，也不向其他节点同步本地 DB 或 artifact。它只从本地 CKB RPC 和 `artifacts/` 读取可验证数据，并暴露 voting 相关 JSON-RPC。

服务输入：

```text
proposal cell
proposal manifest
snapshot file
snapshot root
vote_start_block
vote_end_block
chain/indexer data
```

服务输出：

```text
proposal_id
snapshot_root
valid_votes
invalid_votes
choice_weights
tally_root
verification_report
```

当前服务方法：

```text
tally.get_tip
tally.get_proposals
tally.get_proposal
tally.get_snapshot
tally.get_owner_proof
tally.get_record_proof
tally.get_votes
tally.get_tally
tally.get_voter_options
```

独立 verifier 应该能做到：

1. 从链上找到 proposal cell。
2. 下载 manifest，检查 `manifest_hash`。
3. 在 snapshot block 重建 eligible DAO deposits。
4. 计算 snapshot root，和链上/公开 root 对比。
5. 扫描投票区间内 vote cells。
6. 重新计算 tally，和公开 tally root 对比。

## 开发步骤

### 0. 初始化本地 dev chain

- [x] 创建 `dao-treasury/` 实验目录。
- [x] 运行 `ckb -C dao-treasury init --chain dev ...`。
- [x] 确认 `ckb.toml`、`ckb-miner.toml`、`specs/` 已生成。
- [x] 确认 CKB DB 会写入 `dao-treasury/data/`。
- [x] 打开本地需要的 RPC modules：`Indexer`、`IntegrationTest`。

### 1. 启动和出块

- [x] 启动本地 node。
- [x] 启动 miner，或用 `generate_block` 快速出块。
- [x] 确认 RPC 可用。
- [x] 确认本地链高度增长。

### 2. 准备账户和资金

- [x] 创建或导入 faucet/miner 账户。
- [x] 创建 Alice/Bob/Carol/Proposer/Treasury Recipient。
- [x] 从 faucet 给测试账户转账。
- [x] 等待交易 committed。

### 3. 准备 DAO deposits

- [x] Alice 存入一笔 DAO。
- [x] Bob 存入一笔或多笔 DAO。
- [x] Carol 存入一笔 DAO。
- [x] 记录 deposit out points。
- [x] 编写查询脚本，列出当前 live DAO deposit cells。

### 4. Proposal MVP

- [x] 定义 proposal manifest JSON 格式。
- [x] 生成 manifest hash。
- [x] 创建 proposal cell。
- [x] 编写 proposal discovery 脚本，能从链上列出可投票 proposal。

### 5. Snapshot MVP

- [x] 编写 snapshot generator。
- [x] 输入 snapshot block。
- [x] 扫描 eligible DAO deposit cells。
- [x] 输出轻量 `snapshot.json`、provider-side `source.json`、`index.json` 和 proofs。
- [x] 编写 snapshot verifier，重算 roots，并可从链上重放验证。

### 6. Vote MVP

- [x] 定义 vote cell 数据格式。
- [x] 编写 vote 创建脚本。
- [x] 验证 vote tx 至少包含和 DAO deposit 相同 lock 的 input。
- [x] 支持重复投票时取最后一票。

### 7. Tally MVP

- [x] 扫描 vote window 内的 vote cells。
- [x] 校验 proposal id、snapshot membership、lock ownership proof。
- [x] 计算各 choice 的权重。
- [x] 输出 tally report。
- [x] 编写独立 verifier 重算 tally。

### 8. Rust Tally Service MVP

- [x] 建立 `tally-service/` Rust service crate。
- [x] 提供 JSON-RPC 查询入口。
- [x] 从 CKB RPC 发现 typed proposal / vote cells。
- [x] 从 snapshot index 提供 owner proof / record proof。
- [x] 提供 voter options 查询。
- [x] 通过 service 重算 final tally root。

### 9. Treasury Bucket 后续实验

- [ ] 在文档层明确 treasury bucket 公式。
- [ ] 设计 cellbase bucket 或 claim tx bucket 两种方案的 PoC。
- [ ] 修改 reward / dao accounting / verifier。
- [ ] 增加 consensus tests。

## 当前环境记录

本地 DB 已按最新 type-script session 设计从空 dev chain 重跑。当前链已经完成 funding、DAO deposits、snapshot、typed Proposal Session Cell、typed Vote Cells 和 final tally。

| 项目 | 状态 |
| --- | --- |
| RPC URL | `http://127.0.0.1:8114` |
| dev chain dir | `dao-treasury/` |
| DB dir | `dao-treasury/data/` |
| artifacts dir | `dao-treasury/artifacts/` |
| Tally service | Rust JSON-RPC service，默认 `http://127.0.0.1:8124` |
| governance type code | `script/testdata/always_success`，占位合约 |
| governance type code hash | `0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5` |
| governance type code outpoint | `0xdb82ac45478e61a1e673bc8057b52756ee0c1630d2b4e17e03ebb73935e69da5:0` |
| current tip | block `110` |

## 当前本地数据集

当前数据集：

| Owner | Capacity | Deposit Out Point | Deposit Block |
| --- | ---: | --- | ---: |
| Alice | `30,000` CKB | `0x60e6d520dbdc083d44bc015c1fbc75c8301e52a12ffb10c0b7f26183d01629c3:0` | `21` |
| Bob | `40,000` CKB | `0xb7dea6a1c11c4e8e7c0777f985dff5feadc5b3ac0f5068b5dfcad88b707450f3:0` | `24` |
| Bob | `25,000` CKB | `0x5faaf9043d0e2ba3bba0ee47bd62ef8107e4b1870a176cfbade037c4b3a2cc08:0` | `27` |
| Carol | `20,000` CKB | `0x473e32b1f26d7230ee090dfd013ea7974eb007f1eaa78a7204c254dd2787f4f1:0` | `30` |

### Snapshot Artifact

主线 MVP snapshot 使用 v3 proof-first 格式。snapshot artifacts 由 `create-snapshot.sh` 生成：

```text
snapshot_id = 0xb1918214bf0cab654989ee4c1db74653900c7b06cbdeefb6dad1b68a9e246b7f
snapshot_block = 30
snapshot_block_hash = 0x25ea0907284e4bcb39175abe0109459fec02e2266c1e9488b1cb9282b24c7679
eligible_records = 4
owners = 3
total_capacity = 115,000 CKB
record_map_root = 0xe3e0a750b23386bcad4ca6c4040098da0b579a1800484121f0b949c789bf4d0b
owner_index_root = 0xb43fd4aa12b0eca5275e5ddbaf1989cbecc473ac89f3f4e1dc36a7ea1809b97a
snapshot_root = 0xb1918214bf0cab654989ee4c1db74653900c7b06cbdeefb6dad1b68a9e246b7f
```

```text
artifacts/snapshot-block-30.json          # 轻量 metadata + roots，无 records 数组
artifacts/snapshot-block-30.source.json   # provider-side 完整 records / owner entries
artifacts/snapshot-block-30.index.json    # provider-side proof index
```

snapshot generator 的验证方式：

1. 读取 `snapshot_block` 和 `snapshot_block_hash`。
2. 从本地 CKB RPC 重放 block `0..snapshot_block`。
3. 维护 DAO live cell set：遇到 input 花掉已知 DAO cell 就删除，遇到 DAO type output 就加入。
4. 在 snapshot block 处筛选 deposit phase DAO cells：`output_data = 0x0000000000000000`。
5. 生成两棵 authenticated maps：
   `deposit_out_point_key -> SnapshotRecord` 和 `owner_key -> OwnerEntry`。
6. 重新计算 `record_map_root`、`owner_index_root` 和 `snapshot_root`。
7. 和 snapshot 文件中的 roots 对比。

这个版本验证时仍从 genesis 扫到 snapshot block。后续主网方向应该把同一套状态更新逻辑改成长期运行的增量 indexer，并定期生成 checkpoint。

注意：`snapshot_root` 只承诺 snapshot 本身，不承诺任何 proposal。多个 proposal 可以共享这个 `snapshot_id` / `snapshot_root`。

### Proposal Artifact

当前 typed Proposal Session Cell：

```text
proposal_id = 0x02260888afda53dad09dce0503cf2e39ed0f43655739fe1c1a2ad6ab684e0a62
manifest_hash = 0x11ffd5ba8fc493ddece55eb1617d2c681aaa35c4d5541d6b68dbb1be120c1498
proposal_tx = 0x4f05185c52a47e7413e3e9b84dae7e9e8ee0aa8f0eabf0684366e5147f0cef00
proposal_out_point = 0x4f05185c52a47e7413e3e9b84dae7e9e8ee0aa8f0eabf0684366e5147f0cef00:0
proposal_block = 33
vote_start_block = 50
vote_end_block = 110
```

当前 Proposal Session Cell 的链上形态是：

```text
type script args: 0x00 || proposal_id
data: canonical JSON commitment
```

Proposal Session Cell data 只包含最小链上字段：

```text
proposal_id
manifest_hash
manifest_uri
snapshot_id
snapshot_root
record_map_root
owner_index_root
snapshot_block_number
snapshot_block_hash
choices
vote_start_block
vote_end_block
```

MVP 里的 proposal discovery 只按 Proposal Session Type Script 查询。钱包视图只展示本地可验证的 proposal：type script 必须匹配 `0x00 || proposal_id`，manifest 必须存在且 hash 匹配，snapshot / index 也必须存在。

### Vote Artifacts

当前 MVP vote cell 的链上形态是：

```text
type script args: 0x01 || proposal_id || deposit_out_point_key_hash
data: canonical JSON commitment
```

vote commitment 包含：

```text
proposal_id
snapshot_id
snapshot_root
deposit_out_point
deposit_out_point_key
choice
voter_lock
owner_key
weight_shannons
record_hash
record_proof
vote_id
```

`discover-votes.sh` 当前用于快速发现 typed vote cells，方便本地调试。正式 tally 不依赖 live cell 状态，而是扫 vote window 内的历史区块输出，并按 vote type script prefix 过滤，避免 vote cell 被花掉后投票记录从 tally 视角消失。

当前 typed votes：

| Voter | Deposit | Choice | Weight | Vote Tx | Block |
| --- | --- | --- | ---: | --- | ---: |
| Alice | `0x60e6d520dbdc083d44bc015c1fbc75c8301e52a12ffb10c0b7f26183d01629c3:0` | `yes` | `30,000` CKB | `0x1eb384b9c70b0984fad4cb9bfe19704b6931075714269ef96d768676a47301c8` | `52` |
| Bob | `0xb7dea6a1c11c4e8e7c0777f985dff5feadc5b3ac0f5068b5dfcad88b707450f3:0` | `yes` | `40,000` CKB | `0x74c0e065dcc1c79777b409c8d65ae7d8d6b76f4f30bf07fe9975d3cded08cf74` | `55` |
| Bob | `0x5faaf9043d0e2ba3bba0ee47bd62ef8107e4b1870a176cfbade037c4b3a2cc08:0` | `no` | `25,000` CKB | `0x9e541b626229df9562dedc77e93725f77356db17d7bb5ee7c766c08605e05fc8` | `58` |
| Carol | `0x473e32b1f26d7230ee090dfd013ea7974eb007f1eaa78a7204c254dd2787f4f1:0` | `no` | `20,000` CKB | `0x627a9a2e26ccd8b73113e7fffa94a042ed79bddfc734762a9e29c5576c634899` | `61` |

### Tally Artifact

当前 final tally 使用 historical block scan，扫描完整投票窗口，并按 vote type script prefix 过滤；每个 vote 都通过其内含的 `record_proof` 验证 snapshot membership。

```text
tally_root = 0xdb23a672d79ce10df91377dc6f60f23a0752a71c2392f9072210a2500262a821
tally_file = artifacts/tally-02260888afda-block-110.json
is_final = true
valid_votes = 4
counted_votes = 4
superseded_votes = 0
invalid_votes = 0
yes = 70,000 CKB
no = 45,000 CKB
abstain = 0 CKB
```

tally root 承诺：

```text
proposal_id
snapshot_id
snapshot_root
vote_window
scan_range
is_final
choice_weights_shannons
counted_votes_root
superseded_votes_root
invalid_votes_root
valid / counted / superseded / invalid vote counts
```

重复投票规则在 tally 中实现：同一个 `deposit_out_point_key` 如果出现多笔有效 vote，按 `(block_number, tx_index, output_index)` 取最后一票计入 `counted_votes`，之前的有效票进入 `superseded_votes`。

### Rust Tally Service Smoke Test

Rust Tally service 已基于当前 dev chain 通过 smoke test：

```text
service_tip = 110
proposal_count = 1
alice_eligible_weight = 30,000 CKB
tally_root = 0xdb23a672d79ce10df91377dc6f60f23a0752a71c2392f9072210a2500262a821
yes = 70,000 CKB
no = 45,000 CKB
abstain = 0 CKB
is_final = true
```

这个 `tally_root` 和 Python verifier 生成的 `artifacts/tally-02260888afda-block-110.json` 一致。后续 demo 优先通过 Tally service 查询；Python 脚本保留为低层 artifact generator 和独立 verifier。

## 本地辅助脚本

| 脚本 | 用途 |
| --- | --- |
| `scripts/start-node.sh` | 启动本地 CKB node |
| `scripts/start-node-bg.sh` | 后台启动本地 CKB node，写入 `ckb-node.pid` 和 `logs/ckb-node.stdout.log` |
| `scripts/stop-node.sh` | 停止由 `start-node-bg.sh` 启动的 node |
| `scripts/run-demo.sh` | 重置本地 demo 数据，并从空链完整跑通 funding、DAO deposits、snapshot、proposal、votes、tally service |
| `scripts/install-launch-agent.sh` | 用 macOS `launchd` 常驻启动本地 CKB node |
| `scripts/uninstall-launch-agent.sh` | 停止并卸载本地 CKB LaunchAgent |
| `scripts/launch-agent-status.sh` | 查看本地 CKB LaunchAgent 状态 |
| `scripts/start-miner.sh` | 启动本地 miner |
| `scripts/generate-block.sh` | 调用 `generate_block` 快速出一个空块 |
| `scripts/generate-blocks.sh` | 调用 `generate_block` 快速连续出多个空块 |
| `scripts/status.sh` | 查看 tip block 和 indexer tip |
| `scripts/mine-blocks.sh` | 用 miner 挖指定数量的块，默认 6 个 |
| `scripts/mine-until-committed.sh` | 挖块直到指定 tx committed |
| `scripts/list-accounts.sh` | 列出本地 dev 账户地址和 lock arg |
| `scripts/query-balances.sh` | 查看本地账户余额和 DAO deposits |
| `scripts/query-dao-live-cells.sh` | 用 Indexer 全局查询 DAO live cells；默认只查 deposit phase |
| `scripts/create-snapshot.sh` | 生成 v3 DAO deposit snapshot、source、index；参数是 snapshot block，默认当前 tip |
| `scripts/verify-snapshot.sh` | 从链上重放验证 snapshot roots |
| `scripts/snapshot-record-proof.sh` | 为某个 DAO deposit outpoint 生成 record membership proof |
| `scripts/snapshot-owner-proof.sh` | 为某个 owner lock arg / owner key 生成 owner proof bundle |
| `scripts/verify-record-proof.sh` | 验证单条 record membership proof |
| `scripts/verify-owner-proof.sh` | 验证 owner proof bundle 和其中的 record proofs |
| `scripts/snapshot-dao-deposits.py` | v3 snapshot、index、proof 生成和验证的 Python 实现 |
| `scripts/create-proposal-cell.sh` | 生成 proposal manifest / commitment artifact |
| `scripts/submit-typed-cell.py` | 构造、签名并提交带 custom type script 的单 output tx |
| `scripts/submit-proposal-cell.sh` | 提交 typed Proposal Session Cell |
| `scripts/discover-proposals.sh` | 按 Proposal Session Type Script 从链上发现 proposal cells |
| `scripts/verify-proposal-manifest.sh` | 校验 proposal manifest hash / proposal id / commitment |
| `scripts/proposal.py` | proposal 创建、验证、发现的 Python 实现 |
| `scripts/create-vote-cell.sh` | 生成 vote artifact |
| `scripts/submit-vote-cell.sh` | 提交 typed Vote Cell |
| `scripts/discover-votes.sh` | 按 Vote Type Script 从链上发现并验证投票，适合本地调试 |
| `scripts/voter-options.sh` | 按 voter/account 聚合 DAO live cells、proposal eligibility、latest vote |
| `scripts/vote.py` | vote 创建、发现和校验的 Python 实现 |
| `scripts/voter-options.py` | voter options 查询的 Python 实现 |
| `scripts/tally-proposal.sh` | 按 vote window 历史区块输出生成 tally report |
| `scripts/verify-tally.sh` | 重扫链并验证已有 tally report |
| `scripts/tally.py` | tally 生成和验证的 Python 实现 |
| `scripts/start-tally-service.sh` | 启动 Rust Tally JSON-RPC service |
| `scripts/tally-rpc.sh` | 调用 Rust Tally service 的通用 JSON-RPC 客户端 |
| `tally-service/` | Rust Tally service MVP crate |
| `scripts/fund-accounts.sh` | 从空链复现账户 funding，需 `CONFIRM=1` |
| `scripts/create-dao-deposits.sh` | 从已 funding 链复现 DAO deposits，需 `CONFIRM=1` |

## 常用命令

注意：当前命令使用 type script session 设计。`create-*` 脚本生成 artifact，`submit-*` 脚本把 artifact 作为 typed cell 提交上链。

从空链完整重跑 demo：

```bash
dao-treasury/scripts/run-demo.sh
```

脚本会清理 `dao-treasury/data/`、`logs/` 和旧 artifacts，保留本地 demo accounts，并在最后写出 `artifacts/demo-summary.json`。

生成投票周期 snapshot：

```bash
dao-treasury/scripts/create-snapshot.sh <snapshot-block>
```

验证 snapshot：

```bash
dao-treasury/scripts/verify-snapshot.sh \
  dao-treasury/artifacts/snapshot-block-<N>.json
```

生成并验证 owner proof：

```bash
dao-treasury/scripts/snapshot-owner-proof.sh \
  dao-treasury/artifacts/snapshot-block-<N>.index.json \
  0x7dec345bc7c2e18dbe47e07b362e6ff0d9b00f82

dao-treasury/scripts/verify-owner-proof.sh \
  dao-treasury/artifacts/snapshot-block-<N>.owner-proof-<owner>.json
```

创建并提交 Proposal Session Cell：

```bash
dao-treasury/scripts/create-proposal-cell.sh \
  dao-treasury/artifacts/snapshot-block-<N>.json

dao-treasury/scripts/submit-proposal-cell.sh \
  dao-treasury/artifacts/proposal-<short-id>.json \
  <governance-type-code-tx>:0
```

发现链上 proposal：

```bash
dao-treasury/scripts/discover-proposals.sh
```

验证 proposal manifest：

```bash
dao-treasury/scripts/verify-proposal-manifest.sh \
  dao-treasury/artifacts/proposal-<short-id>.json
```

推进到投票窗口：

```bash
dao-treasury/scripts/generate-blocks.sh 17
```

创建并提交 Vote Cell：

```bash
PROPOSAL_FILE=dao-treasury/artifacts/proposal-<short-id>.json \
SNAPSHOT_FILE=dao-treasury/artifacts/snapshot-block-<N>.json \
dao-treasury/scripts/create-vote-cell.sh \
  0x60e6d520dbdc083d44bc015c1fbc75c8301e52a12ffb10c0b7f26183d01629c3:0 \
  yes

dao-treasury/scripts/submit-vote-cell.sh \
  alice \
  dao-treasury/artifacts/vote-<short-id>.json \
  <governance-type-code-tx>:0
```

发现当前 live vote cells：

```bash
PROPOSAL_FILE=dao-treasury/artifacts/proposal-<short-id>.json \
SNAPSHOT_FILE=dao-treasury/artifacts/snapshot-block-<N>.json \
dao-treasury/scripts/discover-votes.sh
```

查看某个 voter 的 DAO live cells 和可投 proposal：

```bash
dao-treasury/scripts/voter-options.sh alice
```

启动 Rust Tally service：

```bash
dao-treasury/scripts/start-tally-service.sh
```

通过 Tally service 查询 proposal：

```bash
dao-treasury/scripts/tally-rpc.sh \
  tally.get_proposals \
  '{"include_manifest":false}'
```

通过 Tally service 查询 voter options：

```bash
dao-treasury/scripts/tally-rpc.sh \
  tally.get_voter_options \
  '{"voter":"alice"}'
```

通过 Tally service 查询 tally：

```bash
dao-treasury/scripts/tally-rpc.sh \
  tally.get_tally \
  '{"proposal_id":"0x02260888afda53dad09dce0503cf2e39ed0f43655739fe1c1a2ad6ab684e0a62"}'
```

推进到投票结束块：

```bash
dao-treasury/scripts/generate-blocks.sh 48
```

生成 tally report：

```bash
PROPOSAL_FILE=dao-treasury/artifacts/proposal-<short-id>.json \
SNAPSHOT_FILE=dao-treasury/artifacts/snapshot-block-<N>.json \
dao-treasury/scripts/tally-proposal.sh
```

验证 tally report：

```bash
PROPOSAL_FILE=dao-treasury/artifacts/proposal-<short-id>.json \
SNAPSHOT_FILE=dao-treasury/artifacts/snapshot-block-<N>.json \
dao-treasury/scripts/verify-tally.sh \
  dao-treasury/artifacts/tally-<short-id>-block-<N>.json
```
