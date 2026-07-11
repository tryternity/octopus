import { useEffect, useState, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
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
  const createSet = useCallback(async () => {
    const name = prompt('版本名称', '新版本');
    if (!name) return;
    try {
      const id = await invoke<number>('create_hotword_set', { name });
      await refresh();
      setSelectedId(id);
      showToast('已新建版本');
    } catch (e) { showToast('新建失败：' + e); }
  }, [refresh, showToast]);

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
    if (!confirm(`删除版本「${name}」？（命中统计保留）`)) return;
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
    } catch (e) { showToast('添加失败：' + e); }
  }, [input, selectedId, refresh, showToast]);

  const removeWord = useCallback(async (word: string) => {
    if (selectedId === null) return;
    try { await invoke('remove_word_from_set', { id: selectedId, word }); await refresh(); }
    catch (e) { showToast('删除失败：' + e); }
  }, [selectedId, refresh, showToast]);

  // ── 导入 / 导出 / 挖掘 ──
  const doImport = useCallback(async (mode: 'new' | 'append' | 'overwrite') => {
    if (selectedId === null) { showToast('请先选择版本'); return; }
    try {
      if (mode === 'new') {
        const name = prompt('新版本名称', '导入版本');
        if (!name) return;
        const id = await invoke<number>('import_hotwords', { mode, newName: name });
        await refresh(); setSelectedId(id); showToast('已导入为新版本');
      } else if (mode === 'overwrite' && !confirm('覆盖当前版本的全部词？')) {
        return;
      } else {
        await invoke('import_hotwords', { mode, targetSetId: selectedId });
        await refresh(); showToast(mode === 'append' ? '已追加' : '已覆盖');
      }
    } catch (e) { showToast('导入失败：' + e); }
  }, [selectedId, refresh, showToast]);

  const doExport = useCallback(async () => {
    if (selectedId === null) return;
    try { await invoke('export_hotwords', { setId: selectedId }); showToast('已导出'); }
    catch (e) { showToast('导出失败：' + e); }
  }, [selectedId, showToast]);

  const mine = useCallback(async () => {
    if (selectedId === null) { showToast('请先选择目标版本'); return; }
    try {
      const n = await invoke<number>('mine_hotword_candidates_to_set', { targetSetId: selectedId });
      showToast(n > 0 ? `挖掘完成，新增 ${n} 词` : '未发现新候选');
      await refresh();
    } catch (e) { showToast('挖掘失败：' + e); }
  }, [selectedId, refresh, showToast]);

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
              <button onClick={createSet} className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-sm font-medium text-white hover:opacity-90">
                <Plus className="w-4 h-4" /> 新建版本
              </button>
              <button onClick={() => doImport('new')} className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground">
                <Upload className="w-3.5 h-3.5" /> 导入新版本
              </button>
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
                  <div key={w} className="relative rounded-md border border-border bg-background px-3 py-2 pr-7 min-w-[112px] max-w-[200px] hover:border-foreground/25">
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
