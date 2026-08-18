// 打开文件入口共用（spec 2026-08-18-compact-editor-open-files §4）。
// 选择器 filter 提示用（后端才是真相源——拖拽不限扩展名，由 collect_open_tabs 分流）。
export const TEXT_IMAGE_EXTS = [
  "md", "markdown", "txt", "log", "json", "yml", "yaml", "toml", "xml", "csv",
  "html", "htm", "js", "jsx", "ts", "tsx", "py", "rs", "sh", "css", "svg",
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif",
];

/// plugin-dialog open() 返回值归一化为路径数组（null=取消）。
export function normalizeDialogSelection(selected: string | string[] | null): string[] {
  if (selected == null) return [];
  return Array.isArray(selected) ? selected : [selected];
}
