# 标注工具栏抽取（AnnotationToolbar）— 设计规格（spec）

> **状态**：设计阶段（2026-07-26）。
>
> **范围**：把 `Screenshot/index.tsx`（1021 行）与 `RecordAnnotation/index.tsx`（595 行）共有的标注状态 + 工具栏 UI + 位置算法抽取到 `components/Annotation/`，消除 ~250 行重复，根除「位置/行为不一致」的用户反馈。
>
> **不在范围**（推迟到 phase 2）：
> - ImagePreview 的标注工具栏（用 SVG 渲染 + popover 变体 + 混入缩放/OCR/置顶，共性弱，单独评估）
> - 鼠标事件 handler 的统一（mousedown 分支两边控制流不同，强行统一会让代码更难读）

## 0. 背景与决策

### 0.1 问题

用户多次反馈「录屏标注的位置/行为与截图不一致」。代码层证据：

| 重复块 | Screenshot 行号 | RecordAnnotation 行号 | 备注 |
|---|---|---|---|
| 工具属性 state + refs | L40-79 | L48-69 | 几乎逐行一致；`numberCounter` 一边 `useState` 一边 `useRef`，**已存在行为不一致** |
| addAnnotation / undo / redo | L161-185 | L134-159 | 完全一致 |
| 9 个工具按钮 JSX | L905-934 | L519-548 | 完全一致 |
| 工具栏容器 + divider + undo/redo 按钮 | L886-941 | L501-556 | 完全一致 |
| 文字输入 textarea 浮层 | L817-883 | L456-498 | 截图多了「编辑模式」分支，新建模式逻辑一致 |
| mousedown 文字/number/pen/shape 起手 | L322-408 | L226-286 | 一致（截图额外有选区 hitTest 分支）|
| mousemove 绘制 + 标注拖动 | L410-478 | L288-329 | 一致（截图额外有 move/resize 选区分支）|
| mouseup 最小尺寸阈值过滤 | L480-528 | L331-352 | 完全一致（同样的 5×5 / 10px / 2 点阈值）|
| 键盘快捷键 cmd+z/shift+z/Delete | L530-555 | L355-389 | 一致 |
| 工具栏 X clamp 算法 | L744-775 | L426-435 | 算法一致，但截图基于 `sel`，录屏基于 `canvasRect` |

### 0.2 决策（已与用户对齐）

| 决策点 | 选择 | 理由 |
|---|---|---|
| 抽取粒度 | **中粒度**：`useAnnotationState` hook + `<AnnotationToolbar>` 组件 + `computeToolbarPosition` 纯函数 | state 重复是 bug 根源（`numberCounter` 不一致就是证据），但 mousedown 分支两边控制流不同，强行统一得不偿失 |
| 工具栏位置 | **统一纯函数** `computeToolbarPosition(sel, viewport, toolbarW)` | 截图的三选算法（below/above/inside）已踩过坑（见 Screenshot L746-749 注释），稳定；录屏把 `canvasRect` 伪装成 `sel` 即可复用 |
| 迁移节奏 | **分任务**：先 RecordAnnotation（小，验证 hook/组件），再 Screenshot（大，重点回归） | diff 小、定位问题容易、可随时停下 |
| 文件路径 | `components/Annotation/` | 与 `lib/annotation.ts`（纯函数）分离，hook/组件是「带 React 状态的封装」 |
| ImagePreview | **phase 2**，不纳入本轮 | 它用 SVG 渲染 + popover 变体 + 工具栏混入缩放/OCR/置顶，共性弱；强行统一会让抽取组件要支持 3 种变体 |

### 0.3 与已有共享代码的关系

项目里已经共享了：
- `lib/annotation.ts`：`Annotation` / `Tool` 类型 + `drawAnnotation` / `drawAnnotationScaled` / `drawMosaic` / `annBounds` / `hitTestAnnotationPrecise` / `PRESET_COLORS`（**纯函数，本轮不动**）
- `pages/Screenshot/ToolButton.tsx`（19 行）：本轮**合并进 `<AnnotationToolbar>` 内部**，原文件保留作为 ImagePreview 的 phase 2 入口（ImagePreview 自己又复制了一份 ToolButton，phase 2 时一并清理）
- `pages/Screenshot/ToolPropsPopover.tsx`（126 行）：本轮**保留**，作为 `<AnnotationToolbar>` 的子组件引用，不重写

## 1. 架构

### 1.1 新增目录结构

```
crates/desktop/frontend/src/
├── components/
│   └── Annotation/
│       ├── useAnnotationState.ts   # 标注状态 + add/undo/redo hook
│       ├── AnnotationToolbar.tsx   # 工具栏组件（9 工具 + undo/redo + children slot）
│       ├── position.ts             # computeToolbarPosition 纯函数
│       └── index.ts                # re-export
└── lib/
    └── annotation.ts               # 不动（纯函数层）
```

### 1.2 三方依赖关系

```
Screenshot/RecordAnnotation
    │
    ├── lib/annotation.ts（已存在，纯函数）
    │
    └── components/Annotation/（本轮新增）
         ├── useAnnotationState ←── 业务组件用 hook 拿 state + actions
         ├── AnnotationToolbar  ←── 业务组件 render 时嵌入，传 state/actions + 自己的 children
         └── position            ←── 业务组件 render 前算好位置传给 AnnotationToolbar
```

### 1.3 设计原则

1. **hook 返回 state + actions，不返回 UI**：业务组件决定怎么 render
2. **组件接受 children**：截图的 OCR/scroll/save/pin/confirm、录屏的 stop 按钮作为 children 注入，工具栏内部不感知业务命令
3. **位置算法纯函数**：输入 `{x,y,w,h}` + 视口尺寸 + 工具栏实测宽度，输出 `{y, placement}`，业务组件自行决定怎么用
4. **state 用 ref + state 镜像**：高频操作（mouse event handler）读 ref 拿最新值，render 时读 state 触发重渲染

## 2. 接口设计

### 2.1 `useAnnotationState` hook

```typescript
// components/Annotation/useAnnotationState.ts

export interface AnnotationState {
  // ── 工具 ──────────────────────────────────
  tool: Tool;
  setTool: (t: Tool) => void;
  toolRef: React.MutableRefObject<Tool>;  // mouse handler 读 ref 拿最新值

  // ── 工具属性 ──────────────────────────────
  toolColor: string;
  setToolColor: (c: string) => void;
  toolColorRef: React.MutableRefObject<string>;
  toolWidth: number;
  setToolWidth: (n: number) => void;
  toolFontSize: number;
  setToolFontSize: (n: number) => void;
  toolFontSizeRef: React.MutableRefObject<number>;
  toolFilled: boolean;
  setToolFilled: (f: boolean) => void;
  toolFilledRef: React.MutableRefObject<boolean>;
  toolCircleSize: number;
  setToolCircleSize: (n: number) => void;

  // ── 标注数据 ──────────────────────────────
  annotations: Annotation[];
  annotationsRef: React.MutableRefObject<Annotation[]>;
  setAnnotations: React.Dispatch<React.SetStateAction<Annotation[]>>;
  drawingRef: React.MutableRefObject<Annotation | null>;
  addAnnotation: (ann: Annotation) => void;
  undoAnnotation: () => void;
  redoAnnotation: () => void;
  redoAvailable: boolean;
  numberCounter: number;
  numberCounterRef: React.MutableRefObject<number>;
  setNumberCounter: React.Dispatch<React.SetStateAction<number>>;
  selectedAnn: number | null;
  setSelectedAnn: React.Dispatch<React.SetStateAction<number | null>>;

  // ── 浮窗 ──────────────────────────────────
  showPopover: boolean;
  setShowPopover: (b: boolean) => void;
  popoverX: number;
  setPopoverX: (n: number) => void;
}

export function useAnnotationState(): AnnotationState;
```

**统一约定（解决 `numberCounter` 不一致）**：
- `numberCounter` 同时有 state（触发 render）+ ref（undo 闭包读最新值）
- undo 中判断 `removed.number === numberCounterRef.current - 1`（不是 `numberCounter - 1`）
- Screenshot 当前用 `useState` 模式（`undo` 在 render 闭包里读到最新 state）也能工作，但迁移后改为 ref 模式更稳——这是**行为统一**的核心收益

### 2.2 `<AnnotationToolbar>` 组件

```typescript
// components/Annotation/AnnotationToolbar.tsx

export interface AnnotationToolbarProps {
  // ── 注入 hook 返回的 state/actions ─────────
  state: AnnotationState;

  // ── 工具选择回调（业务侧透传事件）─────────
  // 业务侧可在此触发 passthrough 切换（录屏）/ 取消选区 move（截图）
  onToolChange?: (t: Tool) => void;

  // ── 位置（业务组件算好后传入）─────────────
  top: number;
  left: number;  // 已 clamp 到视口

  // ── 容器引用（业务组件用 useLayoutEffect 测量宽度做 clamp）
  toolbarRef?: React.Ref<HTMLDivElement>;

  // ── 业务侧自定义尾部按钮 ─────────────────
  // 截图：OCR / scroll / save / pin / confirm / cancel
  // 录屏：stop
  children?: React.ReactNode;

  // ── 属性浮窗 ──────────────────────────────
  // 由 AnnotationToolbar 内部 render（基于 state.showPopover + state.tool）
  // 但浮窗位置（popoverY）由业务侧算好后通过 popoverY 传入
  popoverY?: number;
}

export function AnnotationToolbar(props: AnnotationToolbarProps): React.ReactElement;
```

**职责边界**：
- AnnotationToolbar 内部 render：9 个工具按钮 + divider + undo/redo 按钮 + `<ToolPropsPopover>`
- 业务组件 render：把 `<AnnotationToolbar>` 放在算好的位置，把业务按钮（OCR/save/stop/...）作为 `children` 传进来

**为什么 popover 不内聚到 AnnotationToolbar**：因为 popover 的 Y 位置依赖 `placement`（below → 工具栏下方；above/inside → 工具栏上方），而 placement 是 `computeToolbarPosition` 算出来的——业务组件已经知道 placement，传 `popoverY` 进来比让组件自己再算一遍干净。

### 2.3 `computeToolbarPosition` 纯函数

```typescript
// components/Annotation/position.ts

export type ToolbarPlacement = "below" | "above" | "inside";

export interface Rect {
  x: number; y: number; w: number; h: number;
}

export interface ToolbarPosition {
  y: number;          // 工具栏 top
  placement: ToolbarPlacement;
  belowOrAbove: boolean;  // popover Y 方向：true=下方，false=上方
}

export const TOOLBAR_H = 44;
export const DOCK_MARGIN = 80;

/**
 * 工具栏位置三选算法（来自 Screenshot L744-781，已踩过坑稳定）：
 *   1. below（默认）：选区下方 8px 处
 *   2. above：选区上方（空间不够下方时）
 *   3. inside：选区内部底部（上下都不够时兜底）
 *
 * @param sel 选区/画布矩形（逻辑像素，相对视口左上角）
 * @param viewportH 视口高度（window.innerHeight）
 * @returns { y, placement, belowOrAbove }
 */
export function computeToolbarPosition(sel: Rect, viewportH: number): ToolbarPosition;

/**
 * 工具栏 X 中心点（基于选区中心 + DOCK_MARGIN clamp）
 */
export function computeToolbarCenterX(sel: Rect, viewportW: number, toolbarW: number): number;
```

**截图调用**：`computeToolbarPosition(sel, window.innerHeight)` — sel 就是用户框选的选区
**录屏调用**：`computeToolbarPosition({ x: canvasRect.ox, y: canvasRect.oy, w: canvasRect.w, h: canvasRect.h }, window.innerHeight)` — canvasRect 伪装成 sel

## 3. 不变量

抽取后必须保持不变的行为（否则就是回归）：

1. **9 个工具按钮的图标/顺序/快捷键/i18n key 完全一致**（截图 == 录屏）
2. **undo/redo/最小尺寸阈值过滤逻辑完全一致**（同样的 5×5 / 10px / 2 点阈值）
3. **工具栏位置三选算法以截图为准**（below 优先 / above / inside 兜底）
4. **工具栏 X clamp 算法以截图为准**（`DOCK_MARGIN=80`，`sel.x + sel.w/2` 中心 + 半宽 clamp）
5. **录屏的 canvasRect 偏移**（mousedown 坐标减 canvasRect.ox/oy）保留在业务侧，hook/组件不感知
6. **录屏的 passthrough 切换**保留在业务侧 `onToolChange` 回调里，组件不感知
7. **截图的选区 hitTest/resize** 保留在业务侧 mousedown handler 里，hook 不感知

## 4. 迁移计划（详见 plan）

| Task | 内容 | 验证 |
|---|---|---|
| 1 | 建 `components/Annotation/`：`useAnnotationState` + `AnnotationToolbar` + `computeToolbarPosition`（新文件）| 单元测试覆盖 position 三选算法 + hook 的 add/undo/redo/numberCounter 行为 |
| 2 | 迁 RecordAnnotation：删重复 state/logic，改用 hook + 组件 | 手动 e2e（开录屏 → 9 工具画标注 → undo/redo → passthrough 切换 → stop）|
| 3 | 迁 Screenshot：删重复 state/logic，改用 hook + 组件 | 手动 e2e（截图 → 9 工具画标注 → undo/redo → 选区 resize/move → OCR/save/pin/confirm）|
| 4（phase 2）| 评估 ImagePreview 是否值得迁移 | 待评估 |

## 5. 测试策略

| 测试目标 | 方式 | 覆盖范围 |
|---|---|---|
| `computeToolbarPosition` 三选 | 单元测试 | below（默认）/ above（下方不够）/ inside（上下都不够，全屏截图场景）|
| `computeToolbarCenterX` clamp | 单元测试 | 选区靠左/右边缘，DOCK_MARGIN 留边 |
| `useAnnotationState` add/undo/redo | 单元测试（用 testing-library renderHook）| 添加标注 / undo 后 redoAvailable=true / redo 恢复 |
| `useAnnotationState` numberCounter 回退 | 单元测试 | undo number 标注时 numberCounter -1（ref 模式，**修复截图当前 state 模式的潜在 bug**）|
| 9 工具按钮 + 快捷键 | 手动 e2e（必做）| 截图 + 录屏两边都过一遍 |
| 工具栏位置 | 手动 e2e | 选区靠上/下/左/右/全屏 5 种位置，工具栏不超出屏幕 |

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| hook 抽取后 Screenshot 的 1000+ 行组件 render 顺序变化导致 cursor/光标行为回归 | 先迁 RecordAnnotation（小，595 行）验证 hook 设计，再迁 Screenshot；每步独立 commit |
| `numberCounter` 从 `useState` 改 ref 模式可能触发未知 bug | 单元测试覆盖「连续 number 标注 → undo → redo → 继续 number」全路径 |
| popover 位置依赖 placement，传递链变长导致 popoverY 错位 | 单元测试 + 手动 e2e 双重验证；popover 内部已有 clamp 兜底 |
| 抽取后 ImagePreview 与新组件 API 不兼容（phase 2 接入困难）| hook 设计上保持 state 外置（不内聚到组件），phase 2 时 ImagePreview 可只接入 hook 不接入 Toolbar |
| 截图 `selectedAnn` 状态依赖 render 闭包，迁移后可能丢失 | 保留 `selectedAnn` 在 hook 里（state + setSelectedAnn），业务侧 useEffect 同步 ref 如有需要 |

## 7. 后续迭代（不在本轮）

- ImagePreview 标注工具栏迁移（phase 2）
- 鼠标事件 handler 的进一步抽取（如 `useAnnotationMouseEvents`，参数化坐标转换 + 选区回调）
- 标注数据持久化（截图的「编辑已有标注」、录屏的「回放标注」）
- 标注模板（保存常用标注组合）
