import { useEffect, useState, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { cn } from '@/lib/utils';
import { Type, Plus, BookMarked, X, Search, Upload, Download, Trash2, Wand2, Check } from 'lucide-react';

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

const DIALECT_OPTIONS: { tok: string; label: string }[] = [
  { tok: 'f/h', label: 'f/h 不分（浮 / 护）' },
  { tok: 'hu/wu', label: 'hu/wu 不分（黄 / 王）' },
  { tok: 'n/l', label: 'n/l 不分（刘 / 牛）' },
  { tok: 'r/l', label: 'r/l 不分（热 / 乐）' },
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
    refresh().catch((e) => showToast('加载失败：' + e));
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

  const totalActiveWords = useMemo(
    () => sets.filter((s) => s.enabled).reduce((n, s) => n + new Set(s.wordsText.split(/\s+/).filter(Boolean)).size, 0),
    [sets],
  );

  // ── 版本操作 ──
  // 新建 / 导入新版本：inline input 输入名（WKWebView 不支持 window.prompt）。
  const commitCreate = useCallback(async () => {
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
      showToast(mode === 'create' ? '已新建版本' : '已导入为新版本');
    } catch (e) { showToast(mode === 'create' ? '新建失败：' + e : '导入失败：' + e); }
  }, [createVal, creating, refresh, showToast]);

  const toggleSet = useCallback(async (id: number, enabled: boolean) => {
    try { await invoke('toggle_hotword_set', { id, enabled }); await refresh(); }
    catch (e) { showToast('切换失败：' + e); }
  }, [refresh, showToast]);

  const startRename = (id: number, cur: string) => { setRenaming(id); setRenameVal(cur); };
  const commitRename = useCallback(async (id: number) => {
    const name = renameVal.trim();
    if (!name) { setRenaming(null); return; }
    try { await invoke('rename_hotword_set', { id, name }); await refresh(); }
    catch (e) { showToast('重命名失败：' + e); }
    setRenaming(null);
  }, [renameVal, refresh, showToast]);

  const deleteSet = useCallback(async (id: number, name: string) => {
    if (!(await confirmDialog(`删除版本「${name}」？（命中统计保留）`, { title: '确认删除', kind: 'warning' }))) return;
    try { await invoke('delete_hotword_set', { id }); await refresh(); }
    catch (e) { showToast('删除失败：' + e); }
  }, [refresh, showToast]);

  // ── 单词操作 ──
  const addWord = useCallback(async () => {
    const w = input.trim();
    if (!w || selectedId === null) return;
    try {
      const added = await invoke<boolean>('add_word_to_set', { id: selectedId, word: w });
      setInput('');
      showToast(added ? '已添加' : '已存在');
      await refresh();
      if (added) flashAdded([w]);
    } catch (e) { showToast('添加失败：' + e); }
  }, [input, selectedId, refresh, flashAdded, showToast]);

  const removeWord = useCallback(async (word: string) => {
    if (selectedId === null) return;
    try { await invoke('remove_word_from_set', { id: selectedId, word }); await refresh(); }
    catch (e) { showToast('删除失败：' + e); }
  }, [selectedId, refresh, showToast]);

  // ── 导入 / 导出 / 挖掘 ──
  const doImport = useCallback(async (mode: 'append' | 'overwrite') => {
    if (selectedId === null) { showToast('请先选择版本'); return; }
    try {
      if (mode === 'overwrite' && !(await confirmDialog('覆盖当前版本的全部词？', { title: '确认覆盖', kind: 'warning' }))) return;
      await invoke('import_hotwords', { mode, targetSetId: selectedId });
      await refresh();
      showToast(mode === 'append' ? '已追加' : '已覆盖');
    } catch (e) { showToast('导入失败：' + e); }
  }, [selectedId, refresh, showToast]);

  const doExport = useCallback(async () => {
    if (selectedId === null) return;
    try { await invoke('export_hotwords', { setId: selectedId }); showToast('已导出'); }
    catch (e) { showToast('导出失败：' + e); }
  }, [selectedId, showToast]);

  // ── 挖掘（先候选后确认，不直接落库）──
  // 点「挖掘」→ 后端扫历史拿候选 → 排除当前版本已有词 → 弹确认面板。
  // 用户在面板里取消勾选 / 手动补词 → 点确认才批量 add_words_to_set。
  const mine = useCallback(async () => {
    if (selectedId === null) { showToast('请先选择目标版本'); return; }
    try {
      const candidates = await invoke<string[]>('list_hotword_candidates');
      const existing = new Set(selected?.wordsText.split(/\s+/).filter(Boolean) ?? []);
      const fresh = candidates.filter((w) => !existing.has(w));
      if (fresh.length === 0) { showToast('未发现新候选'); return; }
      setMinePending({ words: fresh, selected: new Set(fresh) });
    } catch (e) { showToast('挖掘失败：' + e); }
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
      showToast(n > 0 ? `已添加 ${n} 词` : '选中词均已存在');
      await refresh();
      flashAdded(ws);
    } catch (e) { showToast('添加失败：' + e); }
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
        <div className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">语音识别 · 热词纠错</div>
        <h2 className="mt-0.5 text-lg font-semibold tracking-tight">热词管理</h2>
        <p className="mt-1 text-xs text-muted-foreground">按场景管理多版本热词，勾选叠加生效。当前生效词 {totalActiveWords} 个。</p>
      </div>

      {/* 方言模糊 —— 保留 */}
      <Card icon={Type} title="方言模糊">
        <div className="grid grid-cols-2 gap-x-8 gap-y-1 py-1">
          {DIALECT_OPTIONS.map(({ tok, label }) => (
            <div key={tok} className="flex items-center justify-between py-2">
              <span className="text-sm">{label}</span>
              <Toggle on={enabledTokens.has(tok)} onClick={() => toggleDialect(tok)} label={label} />
            </div>
          ))}
        </div>
      </Card>

      {/* 版本管理 */}
      <Card icon={BookMarked} title={`热词版本（${sets.length}）`}>
        {!loaded ? (
          <p className="py-8 text-center text-sm text-muted-foreground">加载中…</p>
        ) : (
          <>
            <div className="flex items-center gap-2 py-2.5">
              {creating ? (
                <input
                  autoFocus
                  value={createVal}
                  onChange={(e) => setCreateVal(e.target.value)}
                  onBlur={commitCreate}
                  onKeyDown={(e) => { if (e.key === 'Enter') commitCreate(); if (e.key === 'Escape') { setCreating(null); setCreateVal(''); } }}
                  placeholder={creating === 'create' ? '版本名称（Enter 确认 / Esc 取消）' : '导入版本名称（Enter 后再选 txt）'}
                  className="flex-1 min-w-0 bg-background border border-voice/50 rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice"
                />
              ) : (
                <>
                  <button onClick={() => { setCreating('create'); setCreateVal(''); }} className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-sm font-medium text-white hover:opacity-90">
                    <Plus className="w-4 h-4" /> 新建版本
                  </button>
                  <button onClick={() => { setCreating('import'); setCreateVal(''); }} className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
                    <Upload className="w-3.5 h-3.5" /> 导入新版本
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
                      onKeyDown={(e) => { if (e.key === 'Enter') commitRename(s.id); if (e.key === 'Escape') setRenaming(null); }}
                      className="flex-1 min-w-0 bg-background border border-voice/50 rounded px-1.5 py-0.5 text-sm outline-none"
                    />
                  ) : (
                    <button
                      onClick={() => { setSelectedId(s.id); startRename(s.id, s.name); }}
                      className={cn('truncate text-sm hover:text-voice', selectedId === s.id && 'font-medium text-voice')}
                      title="点击重命名"
                    >
                      {s.name}
                    </button>
                  )}
                  <span className="font-mono text-[10px] text-muted-foreground/60 flex-shrink-0">
                    {s.wordsText.split(/\s+/).filter(Boolean).length} 词
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
        <Card icon={Plus} title={`${selected.name}（${words.length} 词）`}>
          {/* 单个添加 + 导入追加/覆盖 + 挖掘 */}
          <div className="flex items-center gap-2 py-2.5">
            <input
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && addWord()}
              placeholder="人名 / 地名 / 术语"
              className="flex-1 min-w-0 bg-background border border-border rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice/50"
            />
            <button onClick={addWord} className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-sm font-medium text-white hover:opacity-90">
              <Plus className="w-4 h-4" /> 添加
            </button>
            <button onClick={() => doImport('append')} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground" title="导入追加到当前版本">
              <Upload className="w-3.5 h-3.5" /> 追加
            </button>
            <button onClick={() => doImport('overwrite')} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground" title="导入覆盖当前版本">
              <Upload className="w-3.5 h-3.5" /> 覆盖
            </button>
            <button onClick={mine} className="flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
              <Wand2 className="w-3.5 h-3.5" /> 挖掘
            </button>
          </div>

          {/* 挖掘确认面板：候选默认全选，可取消勾选 / 手动补词，确认才落库 */}
          {minePending && (
            <div className="border-t border-voice/30 bg-voice/5 px-3 py-2.5 space-y-2">
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs text-muted-foreground">
                  已选 {minePending.selected.size}/{minePending.words.length} 个候选，确认后添加
                </span>
                <div className="flex items-center gap-2 flex-shrink-0">
                  <button
                    onClick={() => {
                      const allSel = minePending.selected.size === minePending.words.length;
                      setMinePending({ ...minePending, selected: allSel ? new Set() : new Set(minePending.words) });
                    }}
                    className="text-xs text-muted-foreground hover:text-voice"
                  >
                    {minePending.selected.size === minePending.words.length ? '全不选' : '全选'}
                  </button>
                  <button onClick={() => setMinePending(null)} className="text-xs text-muted-foreground hover:text-foreground">取消</button>
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
                  placeholder="手动补一个词（Enter 加入候选）"
                  className="flex-1 min-w-0 bg-background border border-border rounded px-2 py-1 text-xs outline-none focus:border-voice/50"
                />
                <button onClick={addMineWord} className="rounded border border-border px-2 py-1 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">补词</button>
                <button
                  onClick={commitMine}
                  disabled={minePending.selected.size === 0}
                  className="rounded-md bg-voice px-3 py-1 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
                >
                  添加选中的 {minePending.selected.size} 个
                </button>
              </div>
            </div>
          )}

          {/* 搜索 + 排序 */}
          {words.length > 0 && (
            <div className="flex items-center gap-2 py-2 border-t border-border/40">
              <div className="relative flex-1 min-w-0">
                <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground/50 pointer-events-none" />
                <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索（汉字）" className="w-full bg-background border border-border rounded pl-7 pr-2.5 py-1.5 text-sm outline-none focus:border-voice/50" />
              </div>
              <select value={sort} onChange={(e) => setSort(e.target.value as 'time' | 'alpha' | 'hits')} className={cn(selectClass, 'flex-shrink-0')} aria-label="排序方式">
                <option value="time">默认</option>
                <option value="alpha">字母</option>
                <option value="hits">命中度</option>
              </select>
            </div>
          )}

          {/* 卡片网格（命中数 inline） */}
          {words.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">空版本，添加或导入热词。</p>
          ) : visible.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">无匹配热词</p>
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
