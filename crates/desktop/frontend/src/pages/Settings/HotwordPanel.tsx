import { useEffect, useState, useCallback, useMemo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { cn } from '@/lib/utils';
import { Type, Plus, X, Search, Upload, Download, Trash2, Wand2, ArrowDownWideNarrow, RefreshCw } from 'lucide-react';
import { useT } from '@/lib/i18n';
import { Toggle } from '@/components/ui/toggle';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';

interface HotwordSet {
  id: string;
  name: string;
  enabled: boolean;
  wordsText: string;
  createdAt: string;
  updatedAt: string;
}

interface Props {
  /** app_config.fuzzy_dialect（逗号分隔 token：f/h、hu/wu、n/l、r/l） */
  dialect: string;
  /** app_config.asr_correct——热词纠错总开关（2026-08-01 从系统设置-语音迁入） */
  asrCorrect: boolean;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string) => void;
}

const DIALECT_KEYS: { tok: string; key: string }[] = [
  { tok: 'f/h', key: 'settings.hotword.fH' },
  { tok: 'hu/wu', key: 'settings.hotword.huWu' },
  { tok: 'n/l', key: 'settings.hotword.nL' },
  { tok: 'r/l', key: 'settings.hotword.rL' },
  { tok: 'yun/yong', key: 'settings.hotword.yunYong' },
];

/// 排序选项（图标下拉用）——label 经 i18n key 解析，避免硬编码文案。
const SORT_OPTIONS: { value: 'alpha' | 'hits'; key: string }[] = [
  { value: 'alpha', key: 'settings.hotword.sortAlpha' },
  { value: 'hits', key: 'settings.hotword.sortHit' },
];

export function HotwordPanel({ dialect, asrCorrect, setVal, showToast }: Props) {
  const t = useT();
  const [sets, setSets] = useState<HotwordSet[]>([]);
  const [hits, setHits] = useState<Record<string, number>>({});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  /** fuzzy 搜索结果（null=无搜索显示全部；string[]=命中的词，已按 match_score 降序）。
   *  由 debounce effect 调后端 filter_hotwords_fuzzy 异步填充（复用 matcher::match_score，
   *  汉字+拼音首字母+匹配度排序，与 ActionBar 同款算法）。 */
  const [fuzzyMatches, setFuzzyMatches] = useState<string[] | null>(null);
  const [sort, setSort] = useState<'alpha' | 'hits'>('alpha');
  const [sortOpen, setSortOpen] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameVal, setRenameVal] = useState('');
  const [loaded, setLoaded] = useState(false);
  const [recentlyAdded, setRecentlyAdded] = useState<Set<string>>(new Set());
  const [creating, setCreating] = useState<'create' | 'import' | null>(null);
  const [createVal, setCreateVal] = useState('');
  /** 挖掘确认态：候选词 + 当前勾选集合（不落库，确认后才 add_words_to_set）。
   *  关掉即丢弃；切菜单重进组件卸载也自然清空。2026-08-01 改为浮窗呈现（原内联面板）。 */
  const [minePending, setMinePending] = useState<{ words: string[]; selected: Set<string> } | null>(null);
  const [mineInput, setMineInput] = useState('');
  /** 批量添加浮层（点击「添加」图标弹出 textarea，空白分割批量 add_words_to_set）。 */
  const [addModalOpen, setAddModalOpen] = useState(false);
  const [addModalText, setAddModalText] = useState('');
  // Escape 取消守卫：Escape 置 true → commitCreate/commitRename 吞掉 input 卸载触发的 blur 提交；
  // 每次打开输入（按钮 / startRename）重置 false，防残留误吞下次正常 Enter/失焦提交。
  const createCancelledRef = useRef(false);
  const renameCancelledRef = useRef(false);

  /** 高亮"最近一次新增"的词：直接替换（非累加）。
   *  下次新增 → 替换为新词；切菜单重进 → 组件重挂 state 自然清空。无需定时器。 */
  const flashAdded = useCallback((ws: string[]) => {
    setRecentlyAdded(new Set(ws));
  }, []);

  const refresh = useCallback(async () => {
    const [s, h] = await Promise.all([
      invoke<HotwordSet[]>('list_hotword_sets'),
      invoke<Record<string, number>>('list_hotword_hits'),
    ]);
    setSets(s);
    setHits(h);
    if (s.length > 0 && (selectedId === null || !s.some((x) => x.id === selectedId))) {
      setSelectedId(s[0].id);
    }
    setLoaded(true);
    return s;
  }, [selectedId]);

  useEffect(() => {
    refresh().catch((e) => showToast(t('settings.hotword.loadFailed') + e));
  }, [refresh, showToast]);

  const selected = sets.find((s) => s.id === selectedId) || null;
  const words = useMemo(() => (selected?.wordsText.split(/\s+/).filter(Boolean) ?? []), [selected]);

  // fuzzy 搜索：debounce 120ms 后调后端 filter_hotwords_fuzzy（复用 matcher::match_score，
  // 汉字+拼音首字母+匹配度排序）。空 query → null（显示全部）。
  useEffect(() => {
    const q = query.trim();
    if (!q) { setFuzzyMatches(null); return; }
    let cancelled = false;
    const timer = setTimeout(async () => {
      try {
        const result = await invoke<string[]>('filter_hotwords_fuzzy', { query: q, words });
        if (!cancelled) setFuzzyMatches(result);
      } catch { if (!cancelled) setFuzzyMatches([]); }
    }, 120);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [query, words]);

  // visible：fuzzy 命中的词（null=全部）+ sort 排序。
  // fuzzy 由后端 match_score 算（汉字+拼音+匹配度），已自带 score 降序；
  // 用户切 sort 时按字母/命中度重排覆盖 fuzzy 的 score 顺序。
  const visible = useMemo(() => {
    const arr = fuzzyMatches ?? words;
    return [...arr].sort((a, b) => {
      if (sort === 'hits') return (hits[b] ?? 0) - (hits[a] ?? 0);
      return a.localeCompare(b); // alpha（默认）
    });
  }, [fuzzyMatches, words, sort, hits]);

  // ── 版本操作 ──
  // 新建 / 导入新版本：inline input 输入名（WKWebView 不支持 window.prompt）。
  const commitCreate = useCallback(async () => {
    if (createCancelledRef.current) { createCancelledRef.current = false; return; } // Escape 取消：吞掉卸载触发的 blur
    const name = createVal.trim();
    const mode = creating;
    setCreating(null);
    setCreateVal('');
    if (!name || !mode) return;
    try {
      const id = mode === 'create'
        ? await invoke<string>('create_hotword_set', { name })
        : await invoke<string>('import_hotwords', { mode: 'new', newName: name });
      await refresh();
      setSelectedId(id);
      showToast(mode === 'create' ? t('settings.hotword.newVersion') : t('settings.hotword.importedVersion'));
    } catch (e) { showToast(mode === 'create' ? t('settings.hotword.newFailed') + e : t('settings.hotword.importFailed') + e); }
  }, [createVal, creating, refresh, showToast]);

  const toggleSet = useCallback(async (id: string, enabled: boolean) => {
    try { await invoke('toggle_hotword_set', { id, enabled }); await refresh(); }
    catch (e) { showToast(t('settings.hotword.switchFailed') + e); }
  }, [refresh, showToast]);

  const startRename = (id: string, cur: string) => { renameCancelledRef.current = false; setRenaming(id); setRenameVal(cur); };
  const commitRename = useCallback(async (id: string) => {
    if (renameCancelledRef.current) { renameCancelledRef.current = false; return; } // Escape 取消：吞掉卸载触发的 blur
    const name = renameVal.trim();
    if (!name) { setRenaming(null); return; }
    try { await invoke('rename_hotword_set', { id, name }); await refresh(); }
    catch (e) { showToast(t('settings.hotword.renameFailed') + e); }
    setRenaming(null);
  }, [renameVal, refresh, showToast]);

  const deleteSet = useCallback(async (id: string, name: string) => {
    if (!(await confirmDialog(t('settings.hotword.deleteConfirmMsg', { name }), { title: t('settings.hotword.deleteConfirmTitle'), kind: 'warning' }))) return;
    try { await invoke('delete_hotword_set', { id }); await refresh(); }
    catch (e) { showToast(t('settings.hotword.deleteFailed') + e); }
  }, [refresh, showToast]);

  // ── 单词操作 ──
  /** 批量添加浮层确认：textarea 按任意空白（空格/tab/换行）分割 → add_words_to_set。 */
  const commitAddModal = useCallback(async () => {
    const words = addModalText.split(/\s+/).map((s) => s.trim()).filter(Boolean);
    setAddModalOpen(false);
    setAddModalText('');
    if (words.length === 0 || selectedId === null) return;
    try {
      const n = await invoke<number>('add_words_to_set', { id: selectedId, words });
      showToast(n > 0 ? t('settings.hotword.addedN', { n }) : t('settings.hotword.allExist'));
      await refresh();
      if (n > 0) flashAdded(words.slice(0, n));
    } catch (e) { showToast(t('settings.hotword.addFailed2') + e); }
  }, [addModalText, selectedId, refresh, flashAdded, showToast]);

  const removeWord = useCallback(async (word: string) => {
    if (selectedId === null) return;
    try { await invoke('remove_word_from_set', { id: selectedId, word }); await refresh(); }
    catch (e) { showToast(t('settings.hotword.deleteFailed') + e); }
  }, [selectedId, refresh, showToast]);

  // ── 导入 / 导出 / 挖掘 ──
  const doImport = useCallback(async (mode: 'append' | 'overwrite') => {
    if (selectedId === null) { showToast(t('settings.hotword.selectVersionFirst')); return; }
    try {
      if (mode === 'overwrite' && !(await confirmDialog(t('settings.hotword.overwriteConfirmMsg'), { title: t('settings.hotword.overwriteConfirmTitle'), kind: 'warning' }))) return;
      await invoke('import_hotwords', { mode, targetSetId: selectedId });
      await refresh();
      showToast(mode === 'append' ? t('settings.hotword.appended') : t('settings.hotword.overwritten'));
    } catch (e) { showToast(t('settings.hotword.importFailed2') + e); }
  }, [selectedId, refresh, showToast]);

  const doExport = useCallback(async () => {
    if (selectedId === null) return;
    try { await invoke('export_hotwords', { setId: selectedId }); showToast(t('settings.hotword.exported')); }
    catch (e) { showToast(t('settings.hotword.exportFailed') + e); }
  }, [selectedId, showToast]);

  // ── 挖掘（先候选后确认，不直接落库）──
  // 点「挖掘」→ 后端扫历史拿候选 → 排除当前版本已有词 → 弹确认面板。
  // 用户在面板里取消勾选 / 手动补词 → 点确认才批量 add_words_to_set。
  const mine = useCallback(async () => {
    if (selectedId === null) { showToast(t('settings.hotword.selectTargetFirst')); return; }
    try {
      const candidates = await invoke<string[]>('list_hotword_candidates');
      const existing = new Set(selected?.wordsText.split(/\s+/).filter(Boolean) ?? []);
      const fresh = candidates.filter((w) => !existing.has(w));
      if (fresh.length === 0) { showToast(t('settings.hotword.noNewCandidates')); return; }
      setMinePending({ words: fresh, selected: new Set(fresh) });
    } catch (e) { showToast(t('settings.hotword.mineFailed') + e); }
  }, [selectedId, selected, showToast]);

  const toggleMineSel = (w: string) => {
    if (!minePending) return;
    const next = new Set(minePending.selected);
    if (next.has(w)) next.delete(w); else next.add(w);
    setMinePending({ ...minePending, selected: next });
  };

  const addMineWord = () => {
    const w = mineInput.trim();
    if (!w || !minePending) return;
    setMineInput('');
    // 已在候选清单里就只补勾选，不重复
    if (minePending.words.includes(w)) {
      const selected = new Set(minePending.selected); selected.add(w);
      setMinePending({ ...minePending, selected });
      return;
    }
    setMinePending({
      words: [...minePending.words, w],
      selected: new Set([...minePending.selected, w]),
    });
  };

  const commitMine = useCallback(async () => {
    if (!minePending || selectedId === null) return;
    const ws = [...minePending.selected];
    setMinePending(null);
    if (ws.length === 0) return;
    try {
      const n = await invoke<number>('add_words_to_set', { id: selectedId, words: ws });
      showToast(n > 0 ? t('settings.hotword.addedN', { n }) : t('settings.hotword.allExist'));
      await refresh();
      flashAdded(ws);
    } catch (e) { showToast(t('settings.hotword.addFailed2') + e); }
  }, [minePending, selectedId, refresh, flashAdded, showToast]);

  const toggleDialect = useCallback((tok: string) => {
    const sset = new Set(dialect.split(',').map((s) => s.trim()).filter(Boolean));
    if (sset.has(tok)) sset.delete(tok); else sset.add(tok);
    void setVal('fuzzy_dialect', [...sset].join(','));
  }, [dialect, setVal]);
  const enabledTokens = new Set(dialect.split(',').map((s) => s.trim()));

  return (
    <div className="flex h-full flex-col gap-3">
      {/* ════ 顶部横条：方言模糊 + 热词纠错（全局配置，跨左右栏全宽） ════ */}
      <Card>
        <CardHeader>
          <Type className="h-4 w-4 text-muted-foreground" />
          <CardTitle>{t('settings.hotword.correctSection')}</CardTitle>
          {/* 热词纠错总开关——放头部右侧（仅 toggle，无文字） */}
          <div className="ml-auto" onClick={(e) => e.stopPropagation()}>
            <Toggle
              on={asrCorrect}
              onClick={() => setVal('asr_correct', !asrCorrect)}
              aria-label={t('settings.general.pinyinCorrect')}
            />
          </div>
        </CardHeader>
        <CardContent className="py-2.5">
          {/* 方言模糊（2x4 横排，全局开关） */}
          <div className="grid grid-cols-5 gap-x-4">
            {DIALECT_KEYS.map(({ tok, key }) => {
              const label = t(key);
              return (
                <div key={tok} className="flex items-center justify-between">
                  <span className="text-sm">{label}</span>
                  <Toggle on={enabledTokens.has(tok)} onClick={() => toggleDialect(tok)} aria-label={label} />
                </div>
              );
            })}
          </div>
        </CardContent>
      </Card>

      {/* ════ 下方分栏：左词典列表 + 右热词面板 ════ */}
      <div className="flex min-h-0 flex-1 gap-4">
      {/* ════ 左栏：场景（版本）列表——宽度对齐 Settings sidebar 176px ════ */}
      <div className="flex w-[176px] flex-shrink-0 flex-col rounded-lg border border-border bg-muted/30">
        {/* 列表区 */}
        <div className="flex-1 space-y-0.5 overflow-y-auto p-2">
          {!loaded ? (
            <p className="py-8 text-center text-xs text-muted-foreground">{t('settings.hotword.loading')}</p>
          ) : sets.length === 0 ? (
            <p className="py-8 text-center text-xs text-muted-foreground">{t('settings.hotword.emptyVersion')}</p>
          ) : (
            sets.map((s) => {
              const active = selectedId === s.id;
              const cnt = s.wordsText.split(/\s+/).filter(Boolean).length;
              return (
                <div
                  key={s.id}
                  className={cn(
                    'group relative mx-2 rounded-md px-2.5 py-2 transition-colors cursor-pointer',
                    active ? 'bg-accent' : 'hover:bg-accent/60',
                  )}
                  onClick={() => setSelectedId(s.id)}
                >
                  {/* 选中态左侧 voice 竖条——mx-2 让竖条落在容器灰边上 */}
                  {active && <span className="absolute left-[-10px] top-1.5 bottom-1.5 w-[2px] rounded-full bg-voice" />}
                  {/* 第一行：场景名 + 词数 Badge */}
                  <div className="min-w-0 flex items-center gap-1.5">
                    {renaming === s.id ? (
                      <Input
                        variant="default"
                        size="sm"
                        autoFocus
                        value={renameVal}
                        onChange={(e) => setRenameVal(e.target.value)}
                        onBlur={() => commitRename(s.id)}
                        onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { renameCancelledRef.current = true; setRenaming(null); } }}
                        className="focus:border-voice"
                      />
                    ) : (
                      <>
                        <button
                          onClick={(e) => { e.stopPropagation(); setSelectedId(s.id); startRename(s.id, s.name); }}
                          className={cn('min-w-0 flex-1 truncate text-left text-sm hover:text-voice', active && 'font-medium text-foreground')}
                          title={t('settings.hotword.renameHint')}
                        >
                          {s.name}
                        </button>
                        {cnt > 0 && <Badge variant="muted" size="sm">{cnt}</Badge>}
                      </>
                    )}
                  </div>
                  {/* 第二行：左 Toggle 启用状态点，右 hover 显导出/删除 */}
                  <div className="mt-1.5 flex items-center justify-between">
                    <div onClick={(e) => e.stopPropagation()} className="flex items-center gap-1.5">
                      <Toggle
                        on={s.enabled}
                        onClick={() => toggleSet(s.id, !s.enabled)}
                        aria-label={`启用 ${s.name}`}
                      />
                    </div>
                    <div className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                      <Button variant="ghost" size="icon-sm" onClick={(e) => { e.stopPropagation(); setSelectedId(s.id); doExport(); }} aria-label="导出">
                        <Download />
                      </Button>
                      <Button variant="destructive-ghost" size="icon-sm" onClick={(e) => { e.stopPropagation(); deleteSet(s.id, s.name); }} aria-label="删除版本">
                        <Trash2 />
                      </Button>
                    </div>
                  </div>
                </div>
              );
            })
          )}
        </div>

        {/* 底部：新建 / 导入版本（inline input） */}
        <div className="border-t border-border/60 p-2">
          {creating ? (
            <Input
              variant="default"
              size="full"
              autoFocus
              value={createVal}
              onChange={(e) => setCreateVal(e.target.value)}
              onBlur={commitCreate}
              onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { createCancelledRef.current = true; setCreating(null); setCreateVal(''); } }}
              placeholder={creating === 'create' ? t('settings.hotword.newVersionPlaceholder') : t('settings.hotword.importVersionPlaceholder')}
              className="focus:border-voice"
            />
          ) : (
            <div className="flex gap-1.5">
              <Button
                variant="voice"
                size="sm"
                className="flex-1"
                onClick={() => { createCancelledRef.current = false; setCreating('create'); setCreateVal(''); }}
              >
                <Plus /> {t('settings.hotword.newVersionBtn')}
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => { createCancelledRef.current = false; setCreating('import'); setCreateVal(''); }}
                aria-label={t('settings.hotword.importBtn')}
              >
                <Upload />
              </Button>
            </div>
          )}
        </div>
      </div>

      {/* ════ 右栏：热词版本卡（场景名 + 搜索 + 操作 + 词卡，占满剩余空间） ════ */}
        <Card className="flex min-h-0 flex-1 flex-col">
          {/* 头部：排序图标（左，点击下拉）+ 场景名 + 词数 + 搜索（右）+ 操作图标 */}
          <CardHeader className="gap-2">
            {/* 排序：图标按钮 + 点击下拉（兼作场景名前的视觉锚点） */}
            <div className="relative flex-shrink-0">
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={!selected || words.length === 0}
                onClick={() => setSortOpen((v) => !v)}
                aria-label={t('settings.hotword.sortAlpha')}
                title={t(SORT_OPTIONS.find((o) => o.value === sort)?.key ?? 'settings.hotword.sortAlpha')}
              >
                <ArrowDownWideNarrow className="h-4 w-4" />
              </Button>
              {sortOpen && (
                <>
                  {/* outside-click 关闭 */}
                  <div className="fixed inset-0 z-20" onClick={() => setSortOpen(false)} />
                  <div className="absolute left-0 top-full z-30 mt-1 min-w-[88px] rounded-md border border-border bg-background py-1 shadow-md">
                    {SORT_OPTIONS.map((o) => (
                      <button
                        key={o.value}
                        onClick={() => { setSort(o.value); setSortOpen(false); }}
                        className={cn(
                          'block w-full px-3 py-1.5 text-left text-xs hover:bg-accent',
                          sort === o.value ? 'font-medium text-voice' : 'text-foreground'
                        )}
                      >
                        {t(o.key)}
                      </button>
                    ))}
                  </div>
                </>
              )}
            </div>
            <CardTitle className="flex-shrink-0">{selected ? selected.name : t('settings.hotword.selectVersionFirst')}</CardTitle>
            {selected && (
              <Badge variant="muted" size="sm" className="flex-shrink-0">{words.length}</Badge>
            )}
            {/* 搜索框加宽（flex-1）+ 排序缩短（w-[68px]，项只有 2 字） */}
            <div className="ml-auto flex flex-1 items-center justify-end gap-2">
              <div className="relative min-w-[120px] max-w-[220px] flex-1">
                <Search className="pointer-events-none absolute left-2 top-1/2 z-10 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/50" />
                <Input
                  variant="default"
                  size="full"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder={t('settings.hotword.searchPlaceholder')}
                  disabled={!selected || words.length === 0}
                  className="pl-7"
                />
              </div>
              {/* 操作图标按钮：追加 / 覆盖 / 挖掘 / 添加（纯图标，title 提示） */}
              <div className="flex flex-shrink-0 items-center gap-0.5 border-l border-border/60 pl-1.5">
                <Button variant="ghost" size="icon-sm" disabled={!selected} onClick={() => doImport('append')} title={t('settings.hotword.appendBtn')} aria-label={t('settings.hotword.appendBtn')}>
                  <Upload className="h-4 w-4" />
                </Button>
                <Button variant="ghost" size="icon-sm" disabled={!selected} onClick={() => doImport('overwrite')} title={t('settings.hotword.overwriteBtn')} aria-label={t('settings.hotword.overwriteBtn')}>
                  <RefreshCw className="h-4 w-4" />
                </Button>
                <Button variant="ghost" size="icon-sm" disabled={!selected} onClick={mine} title={t('settings.hotword.mineBtn')} aria-label={t('settings.hotword.mineBtn')}>
                  <Wand2 className="h-4 w-4" />
                </Button>
                <Button variant="voice" size="icon-sm" disabled={!selected} onClick={() => { setAddModalText(''); setAddModalOpen(true); }} title={t('settings.hotword.addBtn')} aria-label={t('settings.hotword.addBtn')}>
                  <Plus className="h-4 w-4" />
                </Button>
              </div>
            </div>
          </CardHeader>

          {/* 词卡网格（占满剩余空间，可滚动） */}
          <div className="min-h-0 flex-1 overflow-y-auto p-4">
            {!selected ? (
              <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.selectVersionFirst')}</p>
            ) : words.length === 0 ? (
              <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.emptyVersion')}</p>
            ) : visible.length === 0 ? (
              <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.noMatch')}</p>
            ) : (
              // grid auto-fill + 1fr：卡片等宽拉满，消除右侧空白，最后一行左对齐
              <div className="grid grid-cols-[repeat(auto-fill,minmax(112px,1fr))] gap-2">
                {visible.map((w) => {
                  const h = hits[w] ?? 0;
                  return (
                    <div key={w} className={cn(
                      'group relative flex items-center justify-center rounded-md border bg-background px-3 py-1.5 transition-colors',
                      recentlyAdded.has(w)
                        ? 'border-voice bg-voice/10'
                        : h > 0
                          ? 'border-success/30 bg-success/5'
                          : 'border-border hover:border-destructive/40'
                    )}>
                      {/* hover 时中间显现红色大 X（覆盖词，点击删除） */}
                      <button
                        onClick={() => removeWord(w)}
                        className="absolute inset-0 z-10 flex items-center justify-center bg-background/80 text-destructive opacity-0 backdrop-blur-[1px] transition-opacity group-hover:opacity-100"
                        aria-label={`删除 ${w}`}
                      >
                        <X className="h-5 w-5" strokeWidth={2.5} />
                      </button>
                      {/* 词 + 命中数同行（底层，hover 时被 X 覆盖） */}
                      <div className="flex w-full items-center gap-1.5">
                        <span className="min-w-0 flex-1 truncate text-sm">{w}</span>
                        <span className={cn(
                          'flex-shrink-0 font-mono text-[10px] tabular-nums',
                          h > 0 ? 'text-success' : 'text-muted-foreground/50'
                        )}>{h}</span>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </Card>
      </div>

      {/* ════ 浮窗：批量添加（textarea，空白分割） ════ */}
      {addModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm" onClick={() => setAddModalOpen(false)}>
          <div className="w-[420px] rounded-lg border border-border bg-background p-4 shadow-xl" onClick={(e) => e.stopPropagation()}>
            <div className="mb-2 flex items-center gap-2">
              <Plus className="h-4 w-4 text-voice" />
              <span className="text-sm font-semibold">{t('settings.hotword.addBtn')}</span>
            </div>
            <textarea
              autoFocus
              value={addModalText}
              onChange={(e) => setAddModalText(e.target.value)}
              placeholder={t('settings.hotword.batchPlaceholder')}
              className="h-32 w-full resize-none rounded-md border border-border bg-background px-3 py-2 text-sm focus:border-voice/50 focus:outline-none focus:ring-2 focus:ring-voice/15"
            />
            <div className="mt-3 flex justify-end gap-2">
              <Button variant="outline" size="sm" onClick={() => { setAddModalOpen(false); setAddModalText(''); }}>{t('settings.hotword.cancel')}</Button>
              <Button variant="voice" size="sm" onClick={commitAddModal} disabled={!addModalText.trim()}>{t('settings.hotword.addBtn')}</Button>
            </div>
          </div>
        </div>
      )}

      {/* ════ 浮窗：挖掘确认（候选词 chip，原内联面板改浮窗） ════ */}
      {minePending && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm" onClick={() => setMinePending(null)}>
          <div className="flex max-h-[80vh] w-[480px] flex-col rounded-lg border border-border bg-background p-4 shadow-xl" onClick={(e) => e.stopPropagation()}>
            <div className="mb-2 flex items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <Wand2 className="h-4 w-4 text-info" />
                <span className="text-sm font-semibold">{t('settings.hotword.mineBtn')}</span>
                <span className="text-xs text-muted-foreground">
                  {t('settings.hotword.pendingHint', { selected: minePending.selected.size, total: minePending.words.length })}
                </span>
              </div>
              <button
                onClick={() => {
                  const allSel = minePending.selected.size === minePending.words.length;
                  setMinePending({ ...minePending, selected: allSel ? new Set() : new Set(minePending.words) });
                }}
                className="text-xs text-muted-foreground hover:text-info"
              >
                {minePending.selected.size === minePending.words.length ? t('settings.hotword.deselectAll') : t('settings.hotword.selectAll')}
              </button>
            </div>
            <div className="flex flex-wrap gap-1.5 overflow-y-auto py-1">
              {minePending.words.map((w) => {
                const on = minePending.selected.has(w);
                return (
                  <button
                    key={w}
                    onClick={() => toggleMineSel(w)}
                    className={cn(
                      'rounded-md border px-2 py-1 text-xs transition-colors',
                      on ? 'border-info bg-info/15 text-info' : 'border-border text-muted-foreground/50 line-through hover:text-muted-foreground'
                    )}
                  >
                    {w}
                  </button>
                );
              })}
            </div>
            <div className="mt-3 flex items-center gap-1.5">
              <Input
                variant="default"
                size="full"
                value={mineInput}
                onChange={(e) => setMineInput(e.target.value)}
                onKeyDown={(e) => { if (e.key === 'Enter') addMineWord(); }}
                placeholder={t('settings.hotword.manualPlaceholder')}
                className="text-xs"
              />
              <Button variant="outline" size="sm" onClick={addMineWord}>{t('settings.hotword.manualBtn')}</Button>
            </div>
            <div className="mt-3 flex justify-end gap-2">
              <Button variant="outline" size="sm" onClick={() => setMinePending(null)}>{t('settings.hotword.cancel')}</Button>
              <Button variant="voice" size="sm" onClick={commitMine} disabled={minePending.selected.size === 0}>
                {t('settings.hotword.addSelectedN', { n: minePending.selected.size })}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
