export type TranslateMode = 'off' | 'manual' | '4s' | '8s' | '12s';

export const TRANSLATE_MODES: TranslateMode[] = ['manual', '4s', '8s', '12s'];

export interface TranslatePopupItem {
  label: string;
  current: boolean;
  name: string;
}

/**
 * 从 DB / ToolbarState 读取的 translate_mode 字符串解析为合法的 TranslateMode。
 * 非法值（含 'off'）回退 'manual'——'off' 仅是前端态，不入库。
 */
export function resolveRememberedTranslateMode(raw: string): TranslateMode {
  return TRANSLATE_MODES.includes(raw as TranslateMode)
    ? raw as TranslateMode
    : 'manual';
}

/**
 * 解析自动档位字符串（'4s' / '8s' / '12s'）为秒数。
 * 非自动档（'manual' / 'off'）返回 null。
 */
export function parseThrottleSeconds(mode: TranslateMode): number | null {
  if (mode === 'off' || mode === 'manual') return null;
  const secs = parseInt(mode, 10);
  return isNaN(secs) ? null : secs;
}

/**
 * 构建翻译模式下拉菜单项列表。
 * labelFn 接收模式字符串返回展示文案（i18n key 的值由调用方传入）。
 */
export function buildTranslatePopupItems(
  currentMode: TranslateMode,
  labelFn: (mode: TranslateMode) => string,
): TranslatePopupItem[] {
  return TRANSLATE_MODES.map(m => ({
    label: labelFn(m),
    current: m === currentMode,
    name: m,
  }));
}
