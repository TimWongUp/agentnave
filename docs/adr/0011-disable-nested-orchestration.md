# ADR-0011：被派出的 CLI 只能使用零工具 AgentNave 服务

- 状态：已接受
- 日期：2026-10-16

Provider CLI 可能继承宿主的 MCP 配置，再次连接 AgentNave；提示词约定无法保证单向编排，而有些 CLI 会过滤自定义环境变量。因此选择在核心与工作室的真正服务入口统一判定，嵌套或无法判定时启动零工具 MCP 服务，通过 instructions 解释原因；不挂载业务 lifespan，不产生目录、文件或端口，中继保持转发职责。

两个 supervisor 在启动 Provider 前，仅在自身环境写入 `AGENTNAVE_SUPERVISOR=<自身 PID>`。服务校验标记仍存活，POSIX 还校验 supervisor 脚本身份，过期标记忽略；macOS 通过 libproc 与 sysctl、Linux 通过 `/proc` 读取祖先链，补足环境被过滤的场景。只识别 Python 进程的脚本位置参数为 `agentnave/supervisor.py` 或 `agentnave/windows_supervisor.py`，避免 prompt 中的路径误伤主控。检测只使用标准库与进程内系统接口，不执行 `ps`。

同用户祖先读取失败即禁用，避免把检测失败当成允许；其他用户祖先的参数不参与检查。Windows 第一版仅检查环境标记 PID 是否存活，不验证脚本、不检查祖先链，标记过滤与 PID 复用可能造成漏判或误判。由树外守护进程启动 MCP 时祖先链信号失效，环境标记也丢失则无法识别。这是正常进程继承下的编排边界，不是同用户恶意进程的安全隔离。
