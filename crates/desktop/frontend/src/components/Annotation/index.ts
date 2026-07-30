// 标注工具栏抽取层 re-export
//
// 用法（业务侧）：
//   import { useAnnotationState, AnnotationToolbar, computeToolbarPosition } from "@/components/Annotation";

export { useAnnotationState } from "./useAnnotationState";
export type { AnnotationState } from "./useAnnotationState";

export { useAnnotationInteraction } from "./useAnnotationInteraction";
export type {
  UseAnnotationInteractionOptions,
  AnnotationInteraction,
  ToolContext,
  TextDraft,
  ClientToNatural,
} from "./useAnnotationInteraction";

export { AnnotationToolbar } from "./AnnotationToolbar";
export type { AnnotationToolbarProps } from "./AnnotationToolbar";

export {
  computeToolbarPosition,
  computeToolbarCenterX,
  TOOLBAR_H,
  DOCK_MARGIN,
} from "./position";
export type { Rect, ToolbarPosition, ToolbarPlacement } from "./position";
