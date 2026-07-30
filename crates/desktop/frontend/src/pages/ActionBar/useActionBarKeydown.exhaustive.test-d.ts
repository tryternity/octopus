/**
 * useActionBarKeydown 穷尽性类型测试（compile-time only）。
 *
 * 这个文件不产生运行时测试（vitest 不执行 .test-d.ts），它的唯一作用是
 * 让 tsc 在每次 `tsc -b` 时验证：useActionBarKeydown 的 switch 穷尽了
 * KeyAction 的所有非 IME 成员。
 *
 * 工作原理：useActionBarKeydown.ts 的 switch 末尾有 `default: const _exhaustive: never = action`。
 * 若有人新增 KeyAction 成员又忘加 case，action 在 default 处类型非 never，tsc 报错。
 * 若有人删了 default，本文件无法捕获（这是已知局限——exhaustiveness 的最终保证是代码评审，
 * 本文件只是把"KeyAction 成员清单"显式化，方便 review 时对照 switch）。
 *
 * 这里用 expect-error 验证：故意构造一个不存在的 KeyAction 类型赋给 never 应报错，
 * 反向证明 never 类型约束确实生效。
 */
import type { KeyAction } from "./keyNavigation";

// 正向：所有合法 KeyAction 成员的 type 字面量（与 switch case 一一对照）
// 若新增成员，下面这个联合需同步加——否则 exhaustive check 在 switch 那侧会报错，
// 提醒开发者两处都要改。
type HandledActionTypes =
  | "passthrough"
  | "ignore"
  | "swallow"
  | "escape-clear-query"
  | "escape-dismiss"
  | "search-tab"
  | "search-nav"
  | "search-enter"
  | "slash-complete"
  | "menu-move"
  | "menu-toggle-layer"
  | "menu-enter"
  | "alt-execute"
  | "alt-goto-sub"
  | "alt-goto-main";

// ime-composing / ime-confirm-enter 在 switch 前用 if return 拦截，不进 switch。
// 两个清单合起来应等于 KeyAction["type"] 的全部成员。下面断言这一点：
type AllActionTypes = HandledActionTypes | "ime-composing" | "ime-confirm-enter";

// 编译期断言：AllActionTypes 与 KeyAction["type"] 互相 assignable。
// 若 KeyAction 加了新成员但本文件没加，下面两行会报错。
const _checkForward: AllActionTypes = null as unknown as KeyAction["type"];
const _checkReverse: KeyAction["type"] = null as unknown as AllActionTypes;
void _checkForward;
void _checkReverse;

// 反向验证 never 约束确实生效：把非 never 的值赋给 never 应报错。
// @ts-expect-error string 不是 never——证明 never 类型约束在工作
const _neverCheck: never = "not-never";
void _neverCheck;
