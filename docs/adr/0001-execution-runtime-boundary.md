# ADR-0001：执行运行时边界

- 状态：已被 ADR-0002 取代
- 日期：2025-02-24
- 取代者：[ADR-0002](0002-lightweight-cli-subagent-adapter.md)

原决定让 AgentNave 承担确定性任务图、Coordinator、Runner、持久化与恢复。实践证明这与 Manager 的规划和编排职责重叠，并带来远超实际需求的状态与发布表面，因此不再作为当前架构合同。
