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
8. tally 程序根据 snapshot 和链上 vote cells 计算结果。
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

1. 链上 proposal cell 存最小元数据和 commitment。
2. 链下 manifest 存完整正文、参数、讨论链接、版本等内容。

链上 proposal cell 至少包含：

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

tally 程序输入：

```text
proposal cell
proposal manifest
snapshot file
snapshot root
vote_start_block
vote_end_block
chain/indexer data
```

tally 输出：

```text
proposal_id
snapshot_root
valid_votes
invalid_votes
choice_weights
tally_root
verification_report
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

### 8. Treasury Bucket 后续实验

- [ ] 在文档层明确 treasury bucket 公式。
- [ ] 设计 cellbase bucket 或 claim tx bucket 两种方案的 PoC。
- [ ] 修改 reward / dao accounting / verifier。
- [ ] 增加 consensus tests。

## 当前环境记录

| 项目 | 状态 |
| --- | --- |
| CKB version | `ckb 0.202.0 (d0a6c95 2025-06-11)` |
| ckb-cli version | `ckb-cli 1.15.0 (8c892a5 2025-06-06)` |
| RPC URL | `http://127.0.0.1:8114` |
| dev chain dir | `dao-treasury/` |
| DB dir | `dao-treasury/data/` |
| genesis hash | `0x7749e4c3e8f80d3897ba52fea27c8a0bc019b3c87895a86f42b75ca7d7eced86` |
| node status | 已通过 macOS `launchd` 启动，RPC `127.0.0.1:8114` |
| miner status | 未启动，当前使用 `generate_block` 快速出块 |
| current tip | block `219` |

## 当前本地数据集

### 账户余额

| 账户 | Lock Arg | 当前状态 |
| --- | --- | --- |
| faucet/miner | `0xc8328aabcd9b9e8e64fbc566c4385c3bdeb219d7` | 约 `20,025,419,947.09481432` CKB，包含本地挖矿奖励 |
| Alice | `0x7dec345bc7c2e18dbe47e07b362e6ff0d9b00f82` | DAO `30,000` CKB，free `69,999.99998161` CKB |
| Bob | `0x977120455b83c232da8520c7db7da7aa29ef0125` | DAO `65,000` CKB，free `54,999.99996323` CKB |
| Carol | `0x9a0e7b573eb5aba438d3853c7a3749bcf7820d04` | DAO `20,000` CKB，free `59,999.99998162` CKB |
| Proposer | `0x02c774d39943cc8e64d6c2c472a89fff9a317054` | total `4,999.99998758` CKB，包含 proposal data cell |
| Treasury Recipient | `0x7a5ab692c338254b7cc5b16a4817c53b77407172` | free `1,000` CKB |

### DAO Deposit Cells

| Owner | Capacity | Deposit Out Point | Deposit Block |
| --- | ---: | --- | ---: |
| Alice | `30,000` CKB | `0x60e6d520dbdc083d44bc015c1fbc75c8301e52a12ffb10c0b7f26183d01629c3:0` | `38` |
| Bob | `40,000` CKB | `0xb7dea6a1c11c4e8e7c0777f985dff5feadc5b3ac0f5068b5dfcad88b707450f3:0` | `44` |
| Bob | `25,000` CKB | `0x5faaf9043d0e2ba3bba0ee47bd62ef8107e4b1870a176cfbade037c4b3a2cc08:0` | `50` |
| Carol | `20,000` CKB | `0xb4a14e0846fd7853b1ca67bb5e0f15c9c2ab13b643632a2457c6bf00a8e507a4:0` | `56` |

### Snapshot Artifact

当前主线 MVP snapshot 使用 v3 proof-first 格式：

```text
snapshot_id = 0xbffa7023fa105796b47694b28be35a205fb8993cc14780d21a316fc4c5f8a176
snapshot_block = 139
snapshot_block_hash = 0xd23a448f5f41c306dc5747484cb97d2a3b10e48b2f63da9a07e810ccc38429d6
eligible_records = 4
owners = 3
total_capacity = 115,000 CKB
record_map_root = 0x155c4a4ceb9736c0e1f1267754bda558fa3384b16684c982ed89ec384d11ce6b
owner_index_root = 0x0f374fa4b4ed1b1ca8484eb0861220404611caf8904a23d71682f3c3d348a063
snapshot_root = 0xbffa7023fa105796b47694b28be35a205fb8993cc14780d21a316fc4c5f8a176
```

snapshot artifacts：

```text
artifacts/snapshot-block-139.json          # 轻量 metadata + roots，无 records 数组
artifacts/snapshot-block-139.source.json   # provider-side 完整 records / owner entries
artifacts/snapshot-block-139.index.json    # provider-side proof index
```

当前 snapshot generator 的验证方式：

1. 读取 `snapshot_block` 和 `snapshot_block_hash`。
2. 从本地 CKB RPC 重放 block `0..snapshot_block`。
3. 维护 DAO live cell set：遇到 input 花掉已知 DAO cell 就删除，遇到 DAO type output 就加入。
4. 在 snapshot block 处筛选 deposit phase DAO cells：`output_data = 0x0000000000000000`。
5. 生成两棵 authenticated maps：
   `deposit_out_point_key -> SnapshotRecord` 和 `owner_key -> OwnerEntry`。
6. 重新计算 `record_map_root`、`owner_index_root` 和 `snapshot_root`。
7. 和 snapshot 文件中的 roots 对比。

这个版本验证时仍从 genesis 扫到 snapshot block。后续主网方向应该把同一套状态更新逻辑改成长期运行的增量 indexer，并定期生成 checkpoint。

注意：`snapshot_root` 现在只承诺 snapshot 本身，不承诺任何 proposal。后续多个 proposal 可以共享这个 `snapshot_id` / `snapshot_root`。

### Proposal Artifact

当前 MVP proposal 使用：

```text
title = DAO Treasury Activation MVP
proposal_id = 0xfca04aaecee406ef101f019cc9dbbc50e9bea747097e5dd3b9d705b585078fbc
manifest_hash = 0x19ef804809117b0af1773c09fbbd8b59f4a5a5f901def0abb5dc0493a61bdb82
snapshot_id = 0xbffa7023fa105796b47694b28be35a205fb8993cc14780d21a316fc4c5f8a176
choices = yes / no / abstain
vote_start_block = 159
vote_end_block = 219
proposal_cell_tx = 0xc55490fc8cf82839076cd0229c73db03e1d0b95f55c0f2c72c94633a78ab475b
proposal_cell_out_point = 0xc55490fc8cf82839076cd0229c73db03e1d0b95f55c0f2c72c94633a78ab475b:0
proposal_cell_block = 142
```

proposal manifest 文件：

```text
artifacts/proposal-fca04aaecee4.json
```

proposal cell data 文件：

```text
artifacts/proposal-fca04aaecee4.cell-data.bin
```

当前链上 proposal cell 的数据格式是：

```text
ASCII prefix: "CKB_GOV_PROPOSAL_V1\n"
payload: canonical JSON commitment
```

commitment 只包含最小链上字段：

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

MVP 里的 proposal discovery 先按 proposer lock + proposal data prefix 查询。这个做法方便本地实验，但不是最终协议形态。钱包视图默认只展示本地可验证的 proposal：manifest 必须存在且 hash 匹配，snapshot / index 也必须存在；调试链上残留或不完整 artifact 时，可以给 `voter-options.py` 加 `--include-incomplete`。后续如果要让任意 proposal 都能被钱包全局发现，应该改成统一 type script、registry cell，或明确的 indexer convention。

### Vote Artifacts

当前 MVP vote cell 的数据格式是：

```text
ASCII prefix: "CKB_GOV_VOTE_V2\n"
payload: canonical JSON commitment
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

当前已经创建并上链 4 笔样例投票：

| Voter | Deposit | Choice | Weight | Vote Tx | Block |
| --- | --- | --- | ---: | --- | ---: |
| Alice | `0x60e6d520dbdc083d44bc015c1fbc75c8301e52a12ffb10c0b7f26183d01629c3:0` | `yes` | `30,000` CKB | `0x2db6f321f47d6be99b59021c559f136abc9ba97959f644bd07d05de9b4013375` | `162` |
| Bob | `0xb7dea6a1c11c4e8e7c0777f985dff5feadc5b3ac0f5068b5dfcad88b707450f3:0` | `yes` | `40,000` CKB | `0xa88a6043d4d65ca9c2ce8ea80cb49dc4bf51afbe5492e9f8a5136b0ad9f2a12b` | `165` |
| Bob | `0x5faaf9043d0e2ba3bba0ee47bd62ef8107e4b1870a176cfbade037c4b3a2cc08:0` | `no` | `25,000` CKB | `0x4258d4041e4cddf56d642e2404dd752782f868d60ab708f3f39a4871a54b45a8` | `168` |
| Carol | `0xb4a14e0846fd7853b1ca67bb5e0f15c9c2ab13b643632a2457c6bf00a8e507a4:0` | `no` | `20,000` CKB | `0xd54930eb8a86c56a66bd47e2e1aeb834b131e458ef50dff765d7a10f6deb1d52` | `171` |

`discover-votes.sh` 当前用于快速发现 still-live vote cells，方便本地调试。正式 tally 不依赖 live cell 状态，而是扫 vote window 内的历史区块输出，避免 vote cell 被花掉后投票记录从 tally 视角消失。

### Tally Artifact

当前 final tally 使用 historical block scan，扫描范围为完整投票窗口 `159..219`。每个 vote 都通过其内含的 `record_proof` 验证 snapshot membership。

```text
tally_root = 0x2d397281cf959a1baaf869fb5d60f4c2fbb5ff10498b9ae498087dcf937dbe72
tally_file = artifacts/tally-fca04aaecee4-block-219.json
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

## 本地辅助脚本

| 脚本 | 用途 |
| --- | --- |
| `scripts/start-node.sh` | 启动本地 CKB node |
| `scripts/start-node-bg.sh` | 后台启动本地 CKB node，写入 `ckb-node.pid` 和 `logs/ckb-node.stdout.log` |
| `scripts/stop-node.sh` | 停止由 `start-node-bg.sh` 启动的 node |
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
| `scripts/create-proposal-cell.sh` | 生成 proposal manifest，并创建链上 proposal commitment cell |
| `scripts/discover-proposals.sh` | 从链上发现 proposer 发布的 proposal cells |
| `scripts/verify-proposal-manifest.sh` | 校验 proposal manifest hash / proposal id / commitment |
| `scripts/proposal.py` | proposal 创建、验证、发现的 Python 实现 |
| `scripts/create-vote-cell.sh` | 生成 vote artifact，并创建链上 vote commitment cell |
| `scripts/discover-votes.sh` | 从当前 live vote cells 中发现并验证投票，适合本地调试 |
| `scripts/voter-options.sh` | 按 voter/account 聚合 DAO live cells、proposal eligibility、latest vote |
| `scripts/vote.py` | vote 创建、发现和校验的 Python 实现 |
| `scripts/voter-options.py` | voter options 查询的 Python 实现 |
| `scripts/tally-proposal.sh` | 按 vote window 历史区块输出生成 tally report |
| `scripts/verify-tally.sh` | 重扫链并验证已有 tally report |
| `scripts/tally.py` | tally 生成和验证的 Python 实现 |
| `scripts/fund-accounts.sh` | 从空链复现账户 funding，需 `CONFIRM=1` |
| `scripts/create-dao-deposits.sh` | 从已 funding 链复现 DAO deposits，需 `CONFIRM=1` |

## 常用命令

生成投票周期 snapshot：

```bash
dao-treasury/scripts/create-snapshot.sh 139
```

验证 snapshot：

```bash
dao-treasury/scripts/verify-snapshot.sh \
  dao-treasury/artifacts/snapshot-block-139.json
```

生成并验证 owner proof：

```bash
dao-treasury/scripts/snapshot-owner-proof.sh \
  dao-treasury/artifacts/snapshot-block-139.index.json \
  0x7dec345bc7c2e18dbe47e07b362e6ff0d9b00f82

dao-treasury/scripts/verify-owner-proof.sh \
  dao-treasury/artifacts/snapshot-block-139.owner-proof-2e86b22e33ef.json
```

创建 proposal cell：

```bash
dao-treasury/scripts/create-proposal-cell.sh \
  dao-treasury/artifacts/snapshot-block-139.json
```

发现链上 proposal：

```bash
dao-treasury/scripts/discover-proposals.sh
```

验证 proposal manifest：

```bash
dao-treasury/scripts/verify-proposal-manifest.sh \
  dao-treasury/artifacts/proposal-fca04aaecee4.json
```

推进到投票窗口：

```bash
dao-treasury/scripts/generate-blocks.sh 17
```

创建 vote cell：

```bash
PROPOSAL_FILE=dao-treasury/artifacts/proposal-fca04aaecee4.json \
SNAPSHOT_FILE=dao-treasury/artifacts/snapshot-block-139.json \
VOTE_CELL_CAPACITY=5000 \
dao-treasury/scripts/create-vote-cell.sh \
  alice \
  0x60e6d520dbdc083d44bc015c1fbc75c8301e52a12ffb10c0b7f26183d01629c3:0 \
  yes
```

发现当前 live vote cells：

```bash
dao-treasury/scripts/discover-votes.sh
```

查看某个 voter 的 DAO live cells 和可投 proposal：

```bash
dao-treasury/scripts/voter-options.sh alice
```

推进到投票结束块：

```bash
dao-treasury/scripts/generate-blocks.sh 48
```

生成 tally report：

```bash
PROPOSAL_FILE=dao-treasury/artifacts/proposal-fca04aaecee4.json \
SNAPSHOT_FILE=dao-treasury/artifacts/snapshot-block-139.json \
dao-treasury/scripts/tally-proposal.sh
```

验证 tally report：

```bash
PROPOSAL_FILE=dao-treasury/artifacts/proposal-fca04aaecee4.json \
SNAPSHOT_FILE=dao-treasury/artifacts/snapshot-block-139.json \
dao-treasury/scripts/verify-tally.sh \
  dao-treasury/artifacts/tally-fca04aaecee4-block-219.json
```
