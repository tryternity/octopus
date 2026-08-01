import { useEffect, useState, useCallback, useMemo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { cn } from '@/lib/utils';
import { Type, Plus, BookMarked, X, Search, Upload, Download, Trash2, Wand2 } from 'lucide-react';
import { useT } from '@/lib/i18n';
import { Toggle } from '@/components/ui/toggle';
import { Input, Select } from '@/components/ui/input';
import { Button } from '@/components/ui/button';

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
];

export function HotwordPanel({ dialect, asrCorrect, setVal, showToast }: Props) {
  const t = useT();
  const [sets, setSets] = useState<HotwordSet[]>([]);
  const [hits, setHits] = useState<Record<string, number>>({});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [input, setInput] = useState('');
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<'time' | 'alpha' | 'hits'>('time');
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameVal, setRenameVal] = useState('');
  const [loaded, setLoaded] = useState(false);
  const [recentlyAdded, setRecentlyAdded] = useState<Set<string>>(new Set());
  const [creating, setCreating] = useState<'create' | 'import' | null>(null);
  const [createVal, setCreateVal] = useState('');
  /** 挖掘确认态：候选词 + 当前勾选集合（不落库，确认后才 add_words_to_set）。
   *  关掉即丢弃；切菜单重进组件卸载也自然清空。 */
  const [minePending, setMinePending] = useState<{ words: string[]; selected: Set<string> } | null>(null);
  const [mineInput, setMineInput] = useState('');
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

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const arr = q ? words.filter((w) => w.toLowerCase().includes(q)) : words;
    return [...arr].sort((a, b) => {
      if (sort === 'hits') return (hits[b] ?? 0) - (hits[a] ?? 0);
      if (sort === 'alpha') return a.localeCompare(b);
      return 0; // time：保留 normalize 后的存储序（拼音首字母序）
    });
  }, [words, query, sort, hits]);

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
  const addWord = useCallback(async () => {
    const w = input.trim();
    if (!w || selectedId === null) return;
    try {
      const added = await invoke<boolean>('add_word_to_set', { id: selectedId, word: w });
      setInput('');
      showToast(added ? t('settings.hotword.added') : t('settings.hotword.exists'));
      await refresh();
      if (added) flashAdded([w]);
    } catch (e) { showToast(t('settings.hotword.addFailed') + e); }
  }, [input, selectedId, refresh, flashAdded, showToast]);

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
    <div className="flex h-full gap-4">
      {/* ════ 左栏：场景（版本）列表 ════ */}
      <div className="flex w-[180px] flex-shrink-0 flex-col rounded-lg border border-border bg-muted/30 raycast-ring">
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
                    'relative rounded-md px-2 py-2 transition-colors cursor-pointer',
                    active ? 'bg-accent' : 'hover:bg-accent/60',
                  )}
                  onClick={() => setSelectedId(s.id)}
                >
                  {/* 选中态左侧 voice 竖条——与 Settings sidebar 一致 */}
                  {active && <span className="absolute left-[-8px] top-1.5 bottom-1.5 w-[2px] rounded-full bg-voice" />}
                  {/* 第一行：场景名称 + 词数（点击重命名） */}
                  <div className="min-w-0">
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
                      <button
                        onClick={(e) => { e.stopPropagation(); setSelectedId(s.id); startRename(s.id, s.name); }}
                        className={cn('block w-full truncate text-left text-[13px] hover:text-voice', active && 'font-medium text-foreground')}
                        title={t('settings.hotword.renameHint')}
                      >
                        {s.name}
                        <span className="ml-1.5 font-mono text-[10px] font-normal text-muted-foreground/60">{cnt} {t('settings.hotword.wordsCount')}</span>
                      </button>
                    )}
                  </div>
                  {/* 第二行：左 Toggle 启用，右对齐 导出/删除。
                      不在外层 stopPropagation——点空白区域仍可切换场景，
                      stopPropagation 移到各控件自身防误触。 */}
                  <div className="mt-1 flex items-center justify-between">
                    <div onClick={(e) => e.stopPropagation()}>
                      <Toggle
                        on={s.enabled}
                        onClick={() => toggleSet(s.id, !s.enabled)}
                        aria-label={`启用 ${s.name}`}
                      />
                    </div>
                    <div className="flex items-center gap-0.5">
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

      {/* ════ 右栏：方言模糊 + 新增热词（上） / 选中场景的词（下） ════ */}
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        {/* ── 右上：所有场景共用的方言模糊 + 新增热词 ── */}
        <div className="rounded-lg border border-border bg-background">
          {/* 热词纠错总开关（2026-08-01 从系统设置-语音迁入——在加热词的地方控制纠错更直观） */}
          <div className="border-b border-border/60 px-4 py-3">
            <div className="flex items-center justify-between">
              <div className="min-w-0">
                <div className="text-[13px]">{t('settings.general.pinyinCorrect')}</div>
                <div className="mt-0.5 text-[11px] text-muted-foreground">{t('settings.general.pinyinCorrectHint')}</div>
              </div>
              <Toggle
                on={asrCorrect}
                onClick={() => setVal('asr_correct', !asrCorrect)}
                aria-label={t('settings.general.pinyinCorrect')}
              />
            </div>
          </div>
          {/* 方言模糊（2x2 grid） */}
          <div className="border-b border-border/60 px-4 py-3">
            <div className="mb-2 flex items-center gap-2">
              <Type className="h-4 w-4 text-muted-foreground" />
              <span className="text-sm font-semibold">{t('settings.hotword.dialectFuzzy')}</span>
            </div>
            <div className="grid grid-cols-2 gap-x-8 gap-y-0.5">
              {DIALECT_KEYS.map(({ tok, key }) => {
                const label = t(key);
                return (
                  <div key={tok} className="flex items-center justify-between py-1.5">
                    <span className="text-[13px]">{label}</span>
                    <Toggle on={enabledTokens.has(tok)} onClick={() => toggleDialect(tok)} aria-label={label} />
                  </div>
                );
              })}
            </div>
          </div>
          {/* 新增热词：input + 添加/追加/覆盖/挖掘（仅选中场景时可用） */}
          <div className="px-4 py-3">
            <div className="mb-2 flex items-center gap-2">
              <Plus className="h-4 w-4 text-muted-foreground" />
              <span className="text-sm font-semibold">
                {selected ? t('settings.hotword.addBtn') : t('settings.hotword.selectVersionFirst')}
              </span>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Input
                variant="default"
                size="full"
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && addWord()}
                placeholder={t('settings.hotword.addPlaceholder')}
                disabled={!selected}
                className="min-w-[160px] flex-1"
              />
              <Button variant="voice" size="default" onClick={addWord} disabled={!selected}>
                <Plus /> {t('settings.hotword.addBtn')}
              </Button>
              <Button variant="outline" size="sm" onClick={() => doImport('append')} disabled={!selected} title={t('settings.hotword.appendHint')}>
                <Upload /> {t('settings.hotword.appendBtn')}
              </Button>
              <Button variant="outline" size="sm" onClick={() => doImport('overwrite')} disabled={!selected} title={t('settings.hotword.overwriteHint')}>
                <Upload /> {t('settings.hotword.overwriteBtn')}
              </Button>
              <Button variant="outline" size="sm" onClick={mine} disabled={!selected}>
                <Wand2 /> {t('settings.hotword.mineBtn')}
              </Button>
            </div>
          </div>
        </div>

        {/* ── 右下：选中场景的词（搜索 + 排序 + 词卡） ── */}
        {selected ? (
          <div className="flex min-h-0 flex-1 flex-col rounded-lg border border-border bg-background">
            {/* 头部：场景名 + 词数 */}
            <div className="flex items-center justify-between border-b border-border/60 px-4 py-2.5">
              <div className="flex items-center gap-2">
                <BookMarked className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-semibold">{selected.name}</span>
                <span className="font-mono text-[10px] text-muted-foreground/60">
                  {words.length} {t('settings.hotword.wordsCount')}
                </span>
              </div>
            </div>

            {/* 挖掘确认面板 */}
            {minePending && (
              <div className="border-b border-voice/30 bg-voice/5 px-3 py-2.5 space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs text-muted-foreground">
                    {t('settings.hotword.pendingHint', { selected: minePending.selected.size, total: minePending.words.length })}
                  </span>
                  <div className="flex flex-shrink-0 items-center gap-2">
                    <button
                      onClick={() => {
                        const allSel = minePending.selected.size === minePending.words.length;
                        setMinePending({ ...minePending, selected: allSel ? new Set() : new Set(minePending.words) });
                      }}
                      className="text-xs text-muted-foreground hover:text-voice"
                    >
                      {minePending.selected.size === minePending.words.length ? t('settings.hotword.deselectAll') : t('settings.hotword.selectAll')}
                    </button>
                    <button onClick={() => setMinePending(null)} className="text-xs text-muted-foreground hover:text-foreground">{t('settings.hotword.cancel')}</button>
                  </div>
                </div>
                <div className="flex max-h-44 flex-wrap gap-1.5 overflow-y-auto">
                  {minePending.words.map((w) => {
                    const on = minePending.selected.has(w);
                    return (
                      <button
                        key={w}
                        onClick={() => toggleMineSel(w)}
                        className={cn(
                          'rounded-md border px-2 py-1 text-xs transition-colors',
                          on ? 'border-voice bg-voice/15 text-voice' : 'border-border text-muted-foreground/50 line-through hover:text-muted-foreground'
                        )}
                      >
                        {w}
                      </button>
                    );
                  })}
                </div>
                <div className="flex items-center gap-1.5">
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
                  <Button
                    variant="voice"
                    size="sm"
                    onClick={commitMine}
                    disabled={minePending.selected.size === 0}
                  >
                    {t('settings.hotword.addSelectedN', { n: minePending.selected.size })}
                  </Button>
                </div>
              </div>
            )}

            {/* 搜索 + 排序 */}
            {words.length > 0 && (
              <div className="flex items-center gap-2 border-b border-border/40 px-4 py-2">
                <div className="relative min-w-0 flex-1">
                  <Search className="pointer-events-none absolute left-2 top-1/2 z-10 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/50" />
                  <Input variant="default" size="full" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t('settings.hotword.searchPlaceholder')} className="pl-7" />
                </div>
                <Select value={sort} onChange={(e) => setSort(e.target.value as 'time' | 'alpha' | 'hits')} className="flex-shrink-0" aria-label="排序方式">
                  <option value="time">{t('settings.hotword.sortDefault')}</option>
                  <option value="alpha">{t('settings.hotword.sortAlpha')}</option>
                  <option value="hits">{t('settings.hotword.sortHit')}</option>
                </Select>
              </div>
            )}

            {/* 词卡网格 */}
            <div className="flex-1 overflow-y-auto p-4">
              {words.length === 0 ? (
                <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.emptyVersion')}</p>
              ) : visible.length === 0 ? (
                <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.noMatch')}</p>
              ) : (
                <div className="flex flex-wrap gap-2">
                  {visible.map((w) => {
                    const h = hits[w] ?? 0;
                    return (
                      <div key={w} className={cn(
                        'raycast-ring relative max-w-[200px] min-w-[112px] rounded-md border bg-background px-3 py-2 pr-7 transition-colors duration-700',
                        recentlyAdded.has(w) ? 'border-voice bg-voice/15 ring-1 ring-voice/30' : 'border-border hover:border-foreground/25'
                      )}>
                        <button onClick={() => removeWord(w)} className="absolute right-1 top-1 rounded p-0.5 text-muted-foreground/60 hover:text-destructive" aria-label={`删除 ${w}`}>
                          <X className="h-3 w-3" />
                        </button>
                        <div className="truncate text-sm">
                          {w} <span className={cn('font-mono text-[10px] tabular-nums', h > 0 ? 'text-voice' : 'text-muted-foreground/50')}>{h}</span>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="flex flex-1 items-center justify-center rounded-lg border border-border bg-background text-sm text-muted-foreground">
            {t('settings.hotword.selectVersionFirst')}
          </div>
        )}
      </div>
    </div>
  );
}
