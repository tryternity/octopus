import * as React from "react";
import { cn } from "@/lib/utils";

/**
 * Card —— 设置面板分组卡片。
 *
 * 替代原本散落在 GeneralPanel/SystemPanel/HotwordPanel 的 3 处本地 Card 定义。
 * 结构：Card（容器）+ CardHeader（图标+标题头）+ CardContent（内容区）。
 *
 * 头部 bg-muted/40 + 底部 border-b 是现有面板统一的视觉约定。
 * raycast 主题下 Card 可叠加 .raycast-ring 获得双环容器深度。
 */
const Card = React.forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement>
>(({ className, ...props }, ref) => (
  <div
    ref={ref}
    className={cn(
      "overflow-hidden rounded-lg border border-border bg-background",
      className,
    )}
    {...props}
  />
));
Card.displayName = "Card";

function CardHeader({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "flex items-center gap-2 border-b border-border bg-muted/40 px-4 py-2.5",
        className,
      )}
      {...props}
    />
  );
}

function CardTitle({
  className,
  ...props
}: React.HTMLAttributes<HTMLHeadingElement>) {
  return (
    <h3
      className={cn("text-sm font-semibold", className)}
      {...props}
    />
  );
}

function CardContent({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("px-4 py-1", className)} {...props} />;
}

export { Card, CardHeader, CardTitle, CardContent };
