import { useEffect, useState, useCallback, useMemo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { cn } from '@/lib/utils';
import { Type, Plus, BookMarked, X, Search, Upload, Download, Trash2, Wand2, Check } from 'lucide-react';
import { useT } from '@/lib/i18n';

interface HotwordSet {
  id: number;
  name: string;
  enabled: boolean;
  wordsText: string;
  createdAt: string;
  updatedAt: string;
}

interface Props {
  /** app_config.fuzzy_dialect（逗号分隔 token：f/h、hu/wu、n/l、r/l） */
  dialect: string;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string) => void;
}

const DIALECT_KEYS: { tok: string; key: string }[] = [
  { tok: 'f/h', key: 'settings.hotword.fH' },
  { tok: 'hu/wu', key: 'settings.hotword.huWu' },
  { tok: 'n/l', key: 'settings.hotword.nL' },
  { tok: 'r/l', key: 'settings.hotword.rL' },
];

const selectClass = 'border border-border rounded-md bg-background px-2.5 py-1.5 text-sm cursor-pointer outline-none focus:border-voice/40 hover:border-foreground/30 transition-colors';

function Card({ icon: Icon, title, children }: { icon: React.ElementType; title: string; children: React.ReactNode }) {
  return (
    <div className="mb-3 border border-border rounded-lg overflow-hidden bg-background">
      <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
        <Icon className="w-4 h-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{title}</h3>
      </div>
      <div className="px-4 py-1">{children}</div>
    </div>
  );
}

function Row({ children }: { children: React.ReactNode }) {
  return <div className="flex items-center justify-between py-2.5 border-b border-border/40 last:border-0 gap-3">{children}</div>;
}

function Toggle({ on, onClick, label }: { on: boolean; onClick: () => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={onClick}
      className={cn('relative w-10 h-[22px] rounded-full transition-colors flex-shrink-0', on ? 'bg-voice' : 'bg-muted-foreground/25')}
    >
      <span className={cn('absolute top-0.5 left-0.5 w-[18px] h-[18px] bg-white rounded-full transition-transform shadow-sm', on && 'translate-x-[18px]')} />
    </button>
  );
}

export function HotwordPanel({ dialect, setVal, showToast }: Props) {
  const t = useT();
  const [sets, setSets] = useState<HotwordSet[]>([]);
  const [hits, setHits] = useState<Record<string, number>>({});
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [input, setInput] = useState('');
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<'time' | 'alpha' | 'hits'>('time');
  const [renaming, setRenaming] = useState<number | null>(null);
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

  // 生效词数 = 所有 enabled 版本词的**全局并集去重**（与后端 list_active_hotword_words 一致）；
  // 旧实现按版本 Set.size 求和会跨版本同词重复计。
  const totalActiveWords = useMemo(() => {
    const active = new Set<string>();
    for (const s of sets) {
      if (s.enabled) s.wordsText.split(/\s+/).filter(Boolean).forEach((w) => active.add(w));
    }
    return active.size;
  }, [sets]);

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
        ? await invoke<number>('create_hotword_set', { name })
        : await invoke<number>('import_hotwords', { mode: 'new', newName: name });
      await refresh();
      setSelectedId(id);
      showToast(mode === 'create' ? t('settings.hotword.newVersion') : t('settings.hotword.importedVersion'));
    } catch (e) { showToast(mode === 'create' ? t('settings.hotword.newFailed') + e : t('settings.hotword.importFailed') + e); }
  }, [createVal, creating, refresh, showToast]);

  const toggleSet = useCallback(async (id: number, enabled: boolean) => {
    try { await invoke('toggle_hotword_set', { id, enabled }); await refresh(); }
    catch (e) { showToast(t('settings.hotword.switchFailed') + e); }
  }, [refresh, showToast]);

  const startRename = (id: number, cur: string) => { renameCancelledRef.current = false; setRenaming(id); setRenameVal(cur); };
  const commitRename = useCallback(async (id: number) => {
    if (renameCancelledRef.current) { renameCancelledRef.current = false; return; } // Escape 取消：吞掉卸载触发的 blur
    const name = renameVal.trim();
    if (!name) { setRenaming(null); return; }
    try { await invoke('rename_hotword_set', { id, name }); await refresh(); }
    catch (e) { showToast(t('settings.hotword.renameFailed') + e); }
    setRenaming(null);
  }, [renameVal, refresh, showToast]);

  const deleteSet = useCallback(async (id: number, name: string) => {
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
    <div className="max-w-[640px]">
      <div className="mb-5">
        <div className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">{t('settings.hotword.title')}</div>
        <h2 className="mt-0.5 text-lg font-semibold tracking-tight">{t('settings.hotword.header')}</h2>
        <p className="mt-1 text-xs text-muted-foreground">{t('settings.hotword.intro', { n: totalActiveWords })}</p>
      </div>

      {/* 方言模糊 —— 保留 */}
      <Card icon={Type} title={t('settings.hotword.dialectFuzzy')}>
        <div className="grid grid-cols-2 gap-x-8 gap-y-1 py-1">
          {DIALECT_KEYS.map(({ tok, key }) => {
            const label = t(key);
            return (
            <div key={tok} className="flex items-center justify-between py-2">
              <span className="text-sm">{label}</span>
              <Toggle on={enabledTokens.has(tok)} onClick={() => toggleDialect(tok)} label={label} />
            </div>
            );
          })}
        </div>
      </Card>

      {/* 版本管理 */}
      <Card icon={BookMarked} title={t('settings.hotword.versionsN', { n: sets.length })}>
        {!loaded ? (
          <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.loading')}</p>
        ) : (
          <>
            <div className="flex items-center gap-2 py-2.5">
              {creating ? (
                <input
                  autoFocus
                  value={createVal}
                  onChange={(e) => setCreateVal(e.target.value)}
                  onBlur={commitCreate}
                  onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { createCancelledRef.current = true; setCreating(null); setCreateVal(''); } }}
                  placeholder={creating === 'create' ? t('settings.hotword.newVersionPlaceholder') : t('settings.hotword.importVersionPlaceholder')}
                  className="flex-1 min-w-0 bg-background border border-voice/50 rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice"
                />
              ) : (
                <>
                  <button onClick={() => { createCancelledRef.current = false; setCreating('create'); setCreateVal(''); }} className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-sm font-medium text-white hover:opacity-90">
                    <Plus className="w-4 h-4" /> {t('settings.hotword.newVersionBtn')}
                  </button>
                  <button onClick={() => { createCancelledRef.current = false; setCreating('import'); setCreateVal(''); }} className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
                    <Upload className="w-3.5 h-3.5" /> {t('settings.hotword.importBtn')}
                  </button>
                </>
              )}
            </div>
            {sets.map((s) => (
              <Row key={s.id}>
                <div className="flex items-center gap-2 flex-1 min-w-0">
                  <Toggle on={s.enabled} onClick={() => toggleSet(s.id, !s.enabled)} label={`启用 ${s.name}`} />
                  {renaming === s.id ? (
                    <input
                      autoFocus
                      value={renameVal}
                      onChange={(e) => setRenameVal(e.target.value)}
                      onBlur={() => commitRename(s.id)}
                      onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { renameCancelledRef.current = true; setRenaming(null); } }}
                      className="flex-1 min-w-0 bg-background border border-voice/50 rounded px-1.5 py-0.5 text-sm outline-none"
                    />
                  ) : (
                    <button
                      onClick={() => { setSelectedId(s.id); startRename(s.id, s.name); }}
                      className={cn('truncate text-sm hover:text-voice', selectedId === s.id && 'font-medium text-voice')}
                      title={t('settings.hotword.renameHint')}
                    >
                      {s.name}
                    </button>
                  )}
                  <span className="font-mono text-[10px] text-muted-foreground/60 flex-shrink-0">
                    {s.wordsText.split(/\s+/).filter(Boolean).length} {t('settings.hotword.wordsCount')}
                  </span>
                </div>
                <div className="flex items-center gap-0.5 flex-shrink-0">
                  <button onClick={() => setSelectedId(s.id)} className="rounded p-1 text-muted-foreground hover:text-foreground" aria-label="选中编辑">
                    <Check className={cn('w-3.5 h-3.5', selectedId === s.id ? 'text-voice' : 'opacity-40')} />
                  </button>
                  <button onClick={doExport} disabled={selectedId !== s.id} className="rounded p-1 text-muted-foreground hover:text-foreground disabled:opacity-30" aria-label="导出">
                    <Download className="w-3.5 h-3.5" />
                  </button>
                  <button onClick={() => deleteSet(s.id, s.name)} className="rounded p-1 text-muted-foreground hover:text-red-500" aria-label="删除版本">
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </Row>
            ))}
          </>
        )}
      </Card>

      {/* 选中版本的词（逐词管理体感） */}
      {selected && (
        <Card icon={Plus} title={`${selected.name}（${words.length} ${t('settings.hotword.wordsCount')}）`}>
          {/* 单个添加 + 导入追加/覆盖 + 挖掘 */}
          <div className="flex items-center gap-2 py-2.5">
            <input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && addWord()}
              placeholder={t('settings.hotword.addPlaceholder')}
              className="flex-1 min-w-0 bg-background border border-border rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice/50"
            />
            <button onClick={addWord} className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-sm font-medium text-white hover:opacity-90">
              <Plus className="w-4 h-4" /> {t('settings.hotword.addBtn')}
            </button>
            <button onClick={() => doImport('append')} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground" title={t('settings.hotword.appendHint')}>
              <Upload className="w-3.5 h-3.5" /> {t('settings.hotword.appendBtn')}
            </button>
            <button onClick={() => doImport('overwrite')} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground" title={t('settings.hotword.overwriteHint')}>
              <Upload className="w-3.5 h-3.5" /> {t('settings.hotword.overwriteBtn')}
            </button>
            <button onClick={mine} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
              <Wand2 className="w-3.5 h-3.5" /> {t('settings.hotword.mineBtn')}
            </button>
          </div>

          {/* 挖掘确认面板：候选默认全选，可取消勾选 / 手动补词，确认才落库 */}
          {minePending && (
            <div className="border-t border-voice/30 bg-voice/5 px-3 py-2.5 space-y-2">
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs text-muted-foreground">
                  {t('settings.hotword.pendingHint', { selected: minePending.selected.size, total: minePending.words.length })}
                </span>
                <div className="flex items-center gap-2 flex-shrink-0">
                  <button
                    onClick={() => {
                      const allSel = minePending.selected.size === minePending.words.length;
                      setMinePending({ ...minePending, selected: allSel ? new Set() : new Set(minePending.words) });
                    }}
                    className="text-xs text-muted-foreground hover:text-voice"
                  >
                    {minePending.selected.size === minePending.words.length ? t('settings.hotword.deselectAll') : t('settings.hotword.selectAll')}
                  </button>
                  <button onClick={() => setMinePending(null)} className="text-xs text-muted-foreground hover:text-foreground">{t('settings.hotword.cancel')}</button></button>
                </div>
              </div>
              <div className="flex flex-wrap gap-1.5 max-h-44 overflow-y-auto">
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
                <input
                  value={mineInput}
                  onChange={(e) => setMineInput(e.target.value)}
                  onKeyDown={(e) => { if (e.key === 'Enter') addMineWord(); }}
                  placeholder={t('settings.hotword.manualPlaceholder')}
                  className="flex-1 min-w-0 bg-background border border-border rounded px-2 py-1 text-xs outline-none focus:border-voice/50"
                />
                <button onClick={addMineWord} className="rounded border border-border px-2 py-1 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">{t('settings.hotword.manualBtn')}</button>
                <button
                  onClick={commitMine}
                  disabled={minePending.selected.size === 0}
                  className="rounded-md bg-voice px-3 py-1 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
                >
                  {t('settings.hotword.addSelectedN', { n: minePending.selected.size })}
                </button>
              </div>
            </div>
          )}

          {/* 搜索 + 排序 */}
          {words.length > 0 && (
            <div className="flex items-center gap-2 py-2 border-t border-border/40">
              <div className="relative flex-1 min-w-0">
                <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground/50 pointer-events-none" />
                <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t('settings.hotword.searchPlaceholder')} className="w-full bg-background border border-border rounded pl-7 pr-2.5 py-1.5 text-sm outline-none focus:border-voice/50" />
              </div>
              <select value={sort} onChange={(e) => setSort(e.target.value as 'time' | 'alpha' | 'hits')} className={cn(selectClass, 'flex-shrink-0')} aria-label="排序方式">
                <option value="time">{t('settings.hotword.sortDefault')}</option>
                <option value="alpha">{t('settings.hotword.sortAlpha')}</option>
                <option value="hits">{t('settings.hotword.sortHit')}</option>
              </select>
            </div>
          )}

          {/* 卡片网格（命中数 inline） */}
          {words.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.emptyVersion')}</p>
          ) : visible.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">{t('settings.hotword.noMatch')}</p>
          ) : (
            <div className="flex flex-wrap gap-2 py-2.5">
              {visible.map((w) => {
                const h = hits[w] ?? 0;
                return (
                  <div key={w} className={cn(
                    'relative rounded-md border bg-background px-3 py-2 pr-7 min-w-[112px] max-w-[200px] transition-colors duration-700',
                    recentlyAdded.has(w) ? 'border-voice bg-voice/15 ring-1 ring-voice/30' : 'border-border hover:border-foreground/25'
                  )}>
                    <button onClick={() => removeWord(w)} className="absolute top-1 right-1 rounded p-0.5 text-muted-foreground/60 hover:text-red-500" aria-label={`删除 ${w}`}>
                      <X className="w-3 h-3" />
                    </button>
                    <div className="text-sm truncate">{w}</div>
                    <div className={cn('mt-1 font-mono text-[10px] tabular-nums', h > 0 ? 'text-voice' : 'text-muted-foreground/50')}>{h}</div>
                  </div>
                );
              })}
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
