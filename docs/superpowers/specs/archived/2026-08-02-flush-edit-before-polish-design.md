# flush-edit 同步：停止录音/润色前强制提交前端编辑

> **日期**：2026-08-02
> **状态**：✅ 已实现（Toggle/InstantStop/HandsFreeStop/PolishNow 四路径同步）
> **bug**：用户编辑 ASR 文本后 2s 内结束录音 → 润色用原始文本（编辑被吞）

## 1. 根因

CM6 编辑器（`AsrEditor.tsx:125`）有 **2s 防抖 commit**——用户停止输入 2s 才提交到后端。结束录音/润色是后端同步触发的，与前端防抖无同步点。用户编辑后 2s 内停止录音 → `doCommit` 还没执行 → 后端 transcript 仍是编辑前 → 润色用原始文本。

## 2. 已有的正确/错误路径

| 触发点 | 是否同步编辑 | 说明 |
|---|---|---|
| `polish_now` 前端按钮 | ✅ | `index.tsx:282` 先 `commit()` 再 invoke |
| **Toggle 停止录音** | ❌ | 后端直接 `finalize_after_stop`，前端防抖未 commit |
| **InstantStop / HandsFreeStop** | ❌ | 同 Toggle（走 stop 路径） |
| **PolishNow（PTT/hotkey 后端发起）** | ❌ | 绕过前端，直接 coordinator |
| **看门狗降级 finalize** | ❌ | 同 Toggle（低频，后续补） |
| `doTranslate` 前端按钮 | ✅（天然） | 前端读编辑器内存文本，不读 transcript |

## 3. 设计：复用 prepare-record 两阶段同步

项目已有的 `prepare-record` 模式（`mod.rs:392-429`）：后端 emit 事件 → 前端 listen 后 invoke 回传 → 后端校验 id → 继续 + 超时兜底。复用此模式做 `flush-edit`。

### 流程

```
后端要读 transcript 前（Toggle 停止 / PolishNow）：
  1. emit("flush-edit", flush_id) + pending_flush = Some(flush_id)
  2. spawn 200ms 看门狗 → Command::FlushTimeout { flush_id }
  3. 等 EditFlushed / FlushTimeout

前端 listen("flush-edit")：
  asrEditorRef.current?.commit()  // 强制提交（清防抖 timer）
  invoke("edit_flushed", { flushId })

后端收到 EditFlushed / FlushTimeout：
  校验 flush_id → 走原逻辑（handle_toggle / handle_polish_now）
```

### 关键约束

- **flush_id**：时间戳，防跨会话/重复（同 prepare_id）
- **超时兜底**：200ms 前端没响应就直接 finalize（编辑可能丢但不卡死）
- **instant 模式**：无编辑器，commit 是 no-op，前端秒回 invoke 不延迟
- **幂等**：非编辑态（无 dirtyRanges）commit 也安全（transcript 不变）

## 4. 触发点改造

### Toggle 停止（mod.rs 主循环）
活跃录音态收到 Toggle → 不直接调 handle_toggle，先 emit flush-edit → 改等 EditFlushed/FlushTimeout 到达后调 handle_toggle。InstantStop/HandsFreeStop 同路径（它们最终也触发停止）。

### PolishNow 后端发起（polish.rs handle_polish_now）
入口前 emit flush-edit → 等 → 继续 polish。前端 `polish_now` 按钮（L282 已 commit）触发的路径不重复 flush（前端已 commit + 前端 invoke 进入后端时编辑已在）。

## 5. 不在范围

- 看门狗降级 finalize（低频，后续补）
- 翻译路径（前端已天然安全）
