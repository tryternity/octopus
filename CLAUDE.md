# Language
尽量以中文形式交互，包括对话回复、注释、文档等。

# Documentation Sync
需求变更后，必须同步更新 superpowers 的 specs 和 plans 文档：
- 规格文档：`docs/superpowers/specs/` — 描述功能设计、架构、接口
- 实施计划：`docs/superpowers/plans/` — 描述实施步骤、任务分解
- 架构概览：`docs/architecture.md` — 项目结构与模块说明

如果变更涉及新功能、架构调整或接口变化，在代码变更完成后（或同时）更新对应文档。

# config 目录
`config/` 是指向 `~/.octopus/` 的软链接，这是实际运行配置目录（不在 git 仓库内，无密钥泄露风险）。

对 `config/` 下文件的读写操作，必须使用绝对路径 `~/.octopus/`（即 `/Users/wudarui/.octopus/`）进行，不要通过 `config/` 相对路径访问：
- 读：`~/.octopus/config.yaml`、`~/.octopus/record.txt` 等
- 写：直接写 `~/.octopus/` 下对应文件
- 原因：`config/` 经符号链接访问时，自动安全分类器无法判断目标在仓库外，可能误判为"向仓库提交密钥"而拦截；用绝对路径 `~/.octopus/` 可避免误拦。
