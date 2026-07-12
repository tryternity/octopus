import type { Tab } from "./index";

/// 把指定 idx 处的 temp tab 升级为正式 clipboard tab（纯函数，便于单测）。
///
/// 背景：「图文编辑」入口打开空白 CompactEditor（temp tab，isTemp=true，不写 DB）。
/// 用户点保存 → 后端 insert_clipboard_text_item 返回新 id → 前端把该 tab 从 temp
/// 升级为正式 clipboard tab：key/source/itemId/isTemp 同步，后续编辑走 update 路径。
/// contrast temp 升级：text 已是译文（doSave 时设 tab.text = translatedText），
/// 升级为 single clipboard 条目，丢弃原文。
export function promoteTempTab(tabs: Tab[], idx: number, newId: number): Tab[] {
  return tabs.map((t, i) => {
    if (i !== idx) return t;
    const isContrast = t.mode === 'contrast';
    return {
      ...t,
      key: `clipboard:${newId}`,
      source: "clipboard",
      itemId: newId,
      itemType: "text",
      isTemp: false,
      mode: isContrast ? 'single' : t.mode,
      originalText: undefined,
      translatedText: undefined,
    };
  });
}
