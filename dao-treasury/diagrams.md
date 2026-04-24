# DAO Treasury MVP 架构图和时序图

这份图描述当前 MVP，而不是最终 hardfork 设计。当前版本的核心思路是：

1. CKB 链只承载可验证的 governance cells：Proposal Session Cell 和 Vote Cell。
2. proposal 正文、snapshot index、proof、tally report 是链下 artifact，但都被链上或公开 root/hash 承诺。
3. Rust Tally service 是便利层，不是可信方。任何人都可以重放链、重算 snapshot、重扫 vote window、重算 tally root。

## MVP 系统分层

```mermaid
flowchart TB
  subgraph Client["用户侧"]
    Wallet["钱包 / 前端"]
    ProposalTool["Proposal 创建工具"]
  end

  subgraph Service["可替换链下服务"]
    Tally["Rust Tally service"]
    SnapshotTool["Snapshot 生成器"]
    Verifier["独立 verifier"]
  end

  subgraph Chain["CKB dev chain"]
    DaoCells["Nervos DAO deposit cells"]
    ProposalCell["Proposal Session Cells"]
    VoteCells["Vote Cells"]
    GovType["Governance type script"]
  end

  subgraph Artifacts["可验证 artifacts"]
    Manifest["proposal manifest"]
    SnapshotIndex["snapshot / index / proofs"]
    TallyReport["tally report"]
  end

  Wallet --> Tally
  Wallet --> VoteCells
  ProposalTool --> ProposalCell
  ProposalTool --> Manifest

  SnapshotTool --> DaoCells
  SnapshotTool --> SnapshotIndex

  Tally --> ProposalCell
  Tally --> VoteCells
  Tally --> SnapshotIndex

  Verifier --> ProposalCell
  Verifier --> VoteCells
  Verifier --> SnapshotIndex
  Verifier --> TallyReport

  ProposalCell --> GovType
  VoteCells --> GovType
```

这张图只表达分层关系，不表达所有数据字段。关键边界是：Tally service 可以下线、换人运营或被多个团队同时运行；它提供的是查询和索引便利，不提供最终可信性。最终可信性来自链上 cells、公开 artifacts 的 hash/root，以及 verifier 的可重算性。

## 核心数据流

```mermaid
flowchart LR
  subgraph Input["输入"]
    Dao["DAO deposits"]
    ManifestDraft["proposal 正文"]
  end

  subgraph Build["链下构建"]
    Snapshot["生成 snapshot root"]
    ProposalBuild["生成 proposal commitment"]
    VoteBuild["生成 vote commitment"]
  end

  subgraph OnChain["链上记录"]
    ProposalCell["Proposal Session Cell"]
    VoteCell["Vote Cell"]
  end

  subgraph Verify["任何人可验证"]
    Tally["重扫 vote window"]
    Report["tally root / report"]
  end

  Dao --> Snapshot
  ManifestDraft --> ProposalBuild
  Snapshot --> ProposalBuild
  ProposalBuild --> ProposalCell
  Snapshot --> VoteBuild
  ProposalCell --> VoteBuild
  VoteBuild --> VoteCell
  ProposalCell --> Tally
  VoteCell --> Tally
  Snapshot --> Tally
  Tally --> Report
```

这个图按时间顺序看：先从 DAO deposits 生成 snapshot，再创建 proposal，然后钱包基于 snapshot proof 创建 vote，最后任何人都可以重扫链得到同一个 tally root。

## 链上和链下数据边界

```mermaid
flowchart TB
  subgraph OnChain["链上最小状态"]
    ProposalType["Proposal Session Cell type"]
    ProposalData["Proposal commitment data"]
    VoteType["Vote Cell type"]
    VoteData["Vote commitment data"]
  end

  subgraph OffChain["链下可验证 artifacts"]
    Manifest["proposal manifest"]
    Snapshot["snapshot metadata"]
    Source["snapshot source records"]
    Index["snapshot proof index"]
    TallyReport["tally report"]
  end

  ProposalData -->|"manifest_hash"| Manifest
  ProposalData -->|"snapshot_id / snapshot_root"| Snapshot
  Snapshot -->|"record_map_root"| Source
  Snapshot -->|"owner_index_root"| Index
  VoteData -->|"record_proof"| Snapshot
  TallyReport -->|"proposal_id / snapshot_root / counted_votes_root"| ProposalData
  TallyReport -->|"validates votes from window"| VoteData
```

因此一个 proposal 的完整含义不是只靠链上 data 表达，而是：

```text
Proposal Session Cell
  + manifest_hash 对应的 manifest
  + snapshot_id / snapshot_root 对应的 snapshot
  + vote window 内的 typed Vote Cells
  + 可重算的 tally report
```

## Demo 端到端流程

```mermaid
sequenceDiagram
  autonumber
  participant Demo as "run-demo.sh"
  participant Node as "CKB dev node"
  participant Accounts as "demo accounts"
  participant Snapshot as "snapshot generator"
  participant Proposal as "proposal tools"
  participant Vote as "vote tools"
  participant Tally as "Rust Tally service"
  participant Verifier as "independent verifier"

  Demo->>Node: reset data and start node
  Demo->>Accounts: fund Alice / Bob / Carol / Proposer
  Accounts->>Node: create DAO deposit transactions
  Demo->>Snapshot: create snapshot at current tip
  Snapshot->>Node: replay blocks and collect live DAO deposits
  Snapshot-->>Demo: snapshot_id / snapshot_root / index
  Demo->>Proposal: create manifest and commitment
  Proposal->>Node: submit typed Proposal Session Cell
  Demo->>Node: mine to vote_start_block
  Demo->>Vote: create Alice / Bob / Carol vote artifacts
  Vote->>Node: submit typed Vote Cells
  Demo->>Node: mine to vote_end_block
  Demo->>Verifier: generate and verify tally artifact
  Verifier->>Node: scan vote window
  Verifier-->>Demo: tally_root and weights
  Demo->>Tally: start JSON-RPC service
  Demo->>Tally: query proposals / voter options / tally
  Tally->>Node: read proposal and vote cells
  Tally-->>Demo: service tally_root matches verifier
```

当前 demo 的最终结果是 `yes = 70,000 CKB`，`no = 45,000 CKB`，service 计算出的 `tally_root` 与独立 verifier 一致。

## Snapshot 生成和验证

```mermaid
sequenceDiagram
  autonumber
  participant Operator as "任意 snapshot operator"
  participant SnapshotTool as "snapshot-dao-deposits.py"
  participant CKB as "CKB RPC / indexer"
  participant Artifacts as "snapshot artifacts"
  participant Verifier as "任意 verifier"

  Operator->>SnapshotTool: choose snapshot_block
  SnapshotTool->>CKB: get block hash at snapshot_block
  loop "block 0..snapshot_block"
    SnapshotTool->>CKB: read block transactions
    SnapshotTool->>SnapshotTool: update DAO live set
  end
  SnapshotTool->>SnapshotTool: keep deposit phase DAO cells only
  SnapshotTool->>SnapshotTool: compute record_map_root
  SnapshotTool->>SnapshotTool: compute owner_index_root
  SnapshotTool->>SnapshotTool: compute snapshot_root
  SnapshotTool->>Artifacts: write snapshot.json / source.json / index.json

  Verifier->>Artifacts: read snapshot.json
  Verifier->>CKB: replay chain to snapshot_block
  Verifier->>Verifier: recompute roots
  Verifier-->>Operator: accept only if roots and block hash match
```

在主网方向，这个流程不应该每次从 genesis 重扫。更实际的做法是让 snapshot/indexer 长期增量维护 DAO live set，并为每个投票周期导出 checkpoint 与 snapshot；但是验证逻辑仍然应该能从可信 checkpoint 或 full replay 重算出来。

## Proposal 创建和发现

```mermaid
sequenceDiagram
  autonumber
  participant Creator as "proposal creator"
  participant Tool as "proposal.py"
  participant Store as "manifest storage"
  participant CKB as "CKB chain"
  participant Wallet as "wallet / frontend"
  participant Tally as "Tally service"

  Creator->>Tool: input title / body / choices / vote window / snapshot_id
  Tool->>Tool: canonicalize manifest
  Tool->>Tool: compute manifest_hash and proposal_id
  Tool->>Store: publish manifest
  Tool->>CKB: submit Proposal Session Cell
  Note over CKB: type args = 0x00 + proposal_id

  Wallet->>Tally: tally.get_proposals
  Tally->>CKB: search cells by governance type prefix 0x00
  Tally->>Store: load manifest by manifest_uri
  Tally->>Tally: verify manifest_hash and proposal_id
  Tally-->>Wallet: return locally verified proposals
```

这里 proposal 正文可以很长，不需要全部上链。链上只需要存 commitment；钱包展示时必须校验 `manifest_hash`，否则就不能把该 proposal 当作已验证内容展示。

## Voter Options 查询

```mermaid
sequenceDiagram
  autonumber
  participant Wallet as "wallet / frontend"
  participant Tally as "Tally service"
  participant CKB as "CKB chain"
  participant Index as "snapshot index"

  Wallet->>Tally: tally.get_voter_options(voter)
  Tally->>CKB: discover verified Proposal Session Cells
  Tally->>Index: load owner proof for voter lock arg
  Tally->>Tally: verify owner proof against snapshot_root
  Tally->>CKB: scan vote window for existing votes
  Tally->>Tally: find latest valid vote per deposit
  Tally-->>Wallet: eligible deposits / weight / already voted status
```

钱包判断一个 DAO live cell 能否投某个 proposal，需要同时满足：

1. deposit 出现在该 proposal 引用的 snapshot record set 中。
2. 对应 record proof 能通过 `snapshot_root` 验证。
3. 当前 voter 能证明自己控制 record 里的 lock。
4. vote block 位于 proposal 的投票窗口内。

## 投票提交

```mermaid
sequenceDiagram
  autonumber
  participant Wallet as "wallet / frontend"
  participant Tally as "Tally service"
  participant CKB as "CKB chain"
  participant Verifier as "tally / verifier"

  Wallet->>Tally: get_record_proof(snapshot_id, deposit_out_point)
  Tally-->>Wallet: record proof and record
  Wallet->>Wallet: build vote commitment
  Wallet->>CKB: submit vote transaction
  Note over CKB: Vote Cell type args = 0x01 + proposal_id + deposit_hash
  Note over CKB: vote tx spends an input with the same lock as DAO deposit record

  Verifier->>CKB: scan vote window
  Verifier->>Verifier: verify vote type args
  Verifier->>Verifier: verify proposal_id and snapshot_root
  Verifier->>Verifier: verify record proof
  Verifier->>Verifier: verify tx ownership input
  Verifier-->>Verifier: count vote as valid
```

当前 MVP 不要求 vote tx 花掉 DAO deposit cell 本身。用户在 snapshot 后取出 DAO deposit，不会影响已经针对该 snapshot 投出的 vote，因为权重在 snapshot block 固定。

## Tally 和独立验证

```mermaid
flowchart TD
  Start["输入 proposal_id"]
  Proposal["读取 Proposal Session Cell"]
  Manifest["校验 manifest_hash"]
  Snapshot["读取并校验 snapshot_root"]
  Scan["扫描 vote_start..vote_end"]
  Filter["按 Vote Cell type prefix 过滤"]
  Validate["验证每张 vote"]
  Latest["同一 deposit 取最后一张有效票"]
  Count["按 choice 汇总权重"]
  Root["计算 tally_root"]
  Compare["和公开 report / service 返回值对比"]

  Start --> Proposal
  Proposal --> Manifest
  Proposal --> Snapshot
  Snapshot --> Scan
  Scan --> Filter
  Filter --> Validate
  Validate --> Latest
  Latest --> Count
  Count --> Root
  Root --> Compare
```

单张 vote 的验证条件：

```mermaid
flowchart LR
  Vote["Vote Cell"]
  TypeArgs["type args 匹配 proposal_id 和 deposit hash"]
  Commitment["data commitment 可 canonical decode"]
  ProposalMatch["proposal_id 匹配"]
  SnapshotMatch["snapshot_id / snapshot_root 匹配"]
  Proof["record proof 属于 snapshot_root"]
  Owner["tx input 证明控制 voter_lock"]
  Window["vote output 位于投票窗口"]
  Valid["valid vote"]

  Vote --> TypeArgs
  Vote --> Commitment
  TypeArgs --> ProposalMatch
  Commitment --> SnapshotMatch
  SnapshotMatch --> Proof
  Proof --> Owner
  Owner --> Window
Window --> Valid
```

## zkVM Settlement PoC

```mermaid
flowchart LR
  subgraph Witness["链下 witness"]
    Proposal["proposal artifact"]
    SnapshotSource["snapshot source records"]
    VoteWitnesses["vote witnesses"]
  end

  subgraph Guest["Rust guest-shaped verifier"]
    SnapshotCheck["重算 snapshot_root"]
    VoteCheck["验证 vote proofs"]
    TallyCheck["重算 tally_root"]
    Settlement["生成 settlement_root"]
  end

  subgraph Public["public inputs"]
    SnapshotRoot["snapshot_root"]
    TallyRoot["tally_root"]
    Window["vote window"]
    Weights["choice weights"]
  end

  Proposal --> VoteCheck
  SnapshotSource --> SnapshotCheck
  VoteWitnesses --> VoteCheck
  SnapshotCheck --> VoteCheck
  VoteCheck --> TallyCheck
  TallyCheck --> Settlement

  SnapshotRoot --> SnapshotCheck
  TallyRoot --> TallyCheck
  Window --> VoteCheck
  Weights --> TallyCheck
```

当前 PoC 已经验证 transcript 内部一致性，并且会重算 witnessed blocks / transactions 的 CKB 原生 commitments。下一步要把 transaction inclusion proof、可信 header anchoring、checkpoint transition 加进 witness，让 settlement proof 能证明这些输入来自 canonical CKB chain。

当前这版相对前一步已经多验证了两层关键约束：除了 vote transaction、`containing_block`、`owner_input_cells` 和带首尾 anchors 的 `header_chain_witness` 之外，guest-shaped verifier 还会用 CKB 原生规则重算 header hash、`transactions_root`、`proposals_hash`、`extra_hash` 和 tx hash。也就是说，它现在不只是验证这些 JSON witness 互相能对上，而是验证它们满足 CKB block / tx 的结构承诺。

## 长期韧性目标和当前 MVP 的差距

```mermaid
flowchart LR
  subgraph MVP["当前 MVP"]
    OneTally["一个本地 Tally service"]
    LocalArtifacts["本地 artifacts"]
    DevChain["本地 dev chain"]
  end

  subgraph Target["长期目标"]
    ManyTally["多个可替换 Tally operators"]
    PublicArtifacts["多源 artifact 发布"]
    NodeModule["可选 node Tally module"]
    PureVerifier["纯 verifier / reproducible binary"]
    Mainnet["CKB mainnet + hardfork treasury logic"]
  end

  OneTally --> ManyTally
  LocalArtifacts --> PublicArtifacts
  OneTally --> NodeModule
  OneTally --> PureVerifier
  DevChain --> Mainnet
```

下一步最重要的工程拆分是把 Rust Tally service 中的“纯验证核心”独立成 library。service、CLI verifier、未来可选 CKB node module 都调用同一个 core，这样可以减少实现分叉，也更容易做到 independently verifiable。
