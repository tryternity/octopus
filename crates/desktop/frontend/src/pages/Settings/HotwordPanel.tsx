import { useEffect, useState, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { cn } from '@/lib/utils';
import { Type, Plus, Sparkles, Check, Trash2, Wand2, Info, BookMarked, X, Search } from 'lucide-react';

interface Hotword {
  id: number;
  word: string;
  status: string;
  source: string;
  hitCount: number;
  createdAt: string;
  /** 拼音首字母串（大写），后端 pinyin_initials 算出，搜索/排序用 */
  initials: string;
}

interface Props {
  /** app_config.fuzzy_dialect（逗号分隔 token：f/h、hu/wu、n/l、r/l） */
  dialect: string;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
  showToast: (msg: string) => void;
}

// 方言模糊选项——token 与后端 hotword::parse_dialect 对齐。
const DIALECT_OPTIONS: { tok: string; label: string }[] = [
  { tok: 'f/h', label: 'f/h 不分（浮 / 护）' },
  { tok: 'hu/wu', label: 'hu/wu 不分（黄 / 王）' },
  { tok: 'n/l', label: 'n/l 不分（刘 / 牛）' },
  { tok: 'r/l', label: 'r/l 不分（热 / 乐）' },
];

// 排序 select 样式——复用 GeneralPanel selectClass 风格。
const selectClass = 'border border-border rounded-md bg-background px-2.5 py-1.5 text-sm cursor-pointer outline-none focus:border-voice/40 hover:border-foreground/30 transition-colors';

// ── Card / Row / Toggle：复用 GeneralPanel 同款，保证设置页视觉一致 ──
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
  return (
    <div className="flex items-center justify-between py-2.5 border-b border-border/40 last:border-0 gap-3">
      {children}
    </div>
  );
}

function Toggle({ on, onClick, label }: { on: boolean; onClick: () => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      onClick={onClick}
      className={cn(
        'relative w-10 h-[22px] rounded-full transition-colors flex-shrink-0',
        on ? 'bg-voice' : 'bg-muted-foreground/25',
      )}
    >
      <span className={cn(
        'absolute top-0.5 left-0.5 w-[18px] h-[18px] bg-white rounded-full transition-transform shadow-sm',
        on && 'translate-x-[18px]',
      )} />
    </button>
  );
}

// ── 来源标签：色点 + 等宽名（手动=品牌橙 / 挖掘=绿，对齐 ActionBarPanel script 色）──
function SourceTag({ source }: { source: string }) {
  const isMined = source === 'mined';
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
      <span className={cn('h-1.5 w-1.5 rounded-full', isMined ? 'bg-emerald-500' : 'bg-voice')} />
      {isMined ? '挖掘' : '手动'}
    </span>
  );
}

export function HotwordPanel({ dialect, setVal, showToast }: Props) {
  const [active, setActive] = useState<Hotword[]>([]);
  const [pending, setPending] = useState<Hotword[]>([]);
  const [input, setInput] = useState('');
  const [mining, setMining] = useState(false);
  const [loaded, setLoaded] = useState(false);
  // 生效热词的搜索与排序（纯前端状态）
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<'time' | 'alpha' | 'hits'>('time');

  const refresh = useCallback(async () => {
    const [act, pend] = await Promise.all([
      invoke<Hotword[]>('list_hotwords', { status: 'active' }),
      invoke<Hotword[]>('list_hotwords', { status: 'pending' }),
    ]);
    setActive(act);
    setPending(pend);
    setLoaded(true);
  }, []);

  useEffect(() => {
    refresh().catch((e) => showToast('加载失败：' + e));
  }, [refresh, showToast]);

  const add = useCallback(async () => {
    const w = input.trim();
    if (!w) return;
    try {
      await invoke('add_hotword', { word: w });
      setInput('');
      showToast('已添加');
      await refresh();
    } catch (e) {
      showToast('添加失败：' + e);
    }
  }, [input, refresh, showToast]);

  const confirm = useCallback(async (id: number) => {
    try {
      await invoke('confirm_pending_hotword', { id });
      showToast('已确认');
      await refresh();
    } catch (e) {
      showToast('确认失败：' + e);
    }
  }, [refresh, showToast]);

  const remove = useCallback(async (id: number) => {
    try {
      await invoke('delete_hotword', { id });
      showToast('已删除');
      await refresh();
    } catch (e) {
      showToast('删除失败：' + e);
    }
  }, [refresh, showToast]);

  const mine = useCallback(async () => {
    setMining(true);
    try {
      const n = await invoke<number>('mine_hotword_candidates');
      showToast(n > 0 ? `挖掘完成，新增 ${n} 条候选` : '未发现新的候选');
      await refresh();
    } catch (e) {
      showToast('挖掘失败：' + e);
    } finally {
      setMining(false);
    }
  }, [refresh, showToast]);

  // 勾选/取消某方言组 → 重算逗号分隔串写回 app_config.fuzzy_dialect。
  const toggleDialect = useCallback((tok: string) => {
    const set = new Set(dialect.split(',').map((s) => s.trim()).filter(Boolean));
    if (set.has(tok)) set.delete(tok);
    else set.add(tok);
    void setVal('fuzzy_dialect', [...set].join(','));
  }, [dialect, setVal]);

  const enabledTokens = new Set(dialect.split(',').map((s) => s.trim()));

  // 生效热词：拼音首字母前缀 OR 汉字包含 → 过滤；再按所选键排序。
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? active.filter((h) =>
          h.word.toLowerCase().includes(q) || h.initials.toLowerCase().startsWith(q),
        )
      : active;
    return [...filtered].sort((a, b) => {
      if (sort === 'hits') return b.hitCount - a.hitCount;       // 命中度降序
      if (sort === 'alpha') return a.initials.localeCompare(b.initials); // 字母（拼音首字母）升序
      return b.createdAt.localeCompare(a.createdAt);             // 时间降序（默认）
    });
  }, [active, query, sort]);

  return (
    <div className="max-w-[640px]">
      {/* 页头 */}
      <div className="mb-5">
        <div className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">
          语音识别 · 热词纠错
        </div>
        <h2 className="mt-0.5 text-lg font-semibold tracking-tight">热词管理</h2>
        <p className="mt-1 text-xs text-muted-foreground">
          为常被误识别的专名（人名 / 地名 / 术语）建立纠错词典，识别后自动校正。当前 {active.length} 个生效词。
        </p>
      </div>

      {/* 方言模糊 —— 一行两列 */}
      <Card icon={Type} title="方言模糊">
        <div className="grid grid-cols-2 gap-x-8 gap-y-1 py-1">
          {DIALECT_OPTIONS.map(({ tok, label }) => (
            <div key={tok} className="flex items-center justify-between py-2">
              <span className="text-sm">{label}</span>
              <Toggle on={enabledTokens.has(tok)} onClick={() => toggleDialect(tok)} label={label} />
            </div>
          ))}
        </div>
        <div className="flex items-start gap-1.5 py-2.5 mt-1 text-xs text-muted-foreground/70 border-t border-border/40">
          <Info className="w-3.5 h-3.5 mt-px flex-shrink-0" />
          <span>基础规则（平翘舌 + 前后鼻音）始终开；勾选后按对应口音扩大召回。r/l 仅救首字（如「热→乐」），第二字 sh/c 不归一。</span>
        </div>
      </Card>

      {/* 添加热词 */}
      <Card icon={Plus} title="添加热词">
        <div className="flex items-center gap-2 py-2.5">
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && add()}
            placeholder="人名 / 地名 / 术语 / 口头禅"
            className="flex-1 min-w-0 bg-background border border-border rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice/50"
          />
          <button
            onClick={add}
            className="flex items-center gap-1.5 rounded-md bg-voice px-3.5 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90 flex-shrink-0"
          >
            <Plus className="w-4 h-4" /> 添加
          </button>
          <button
            onClick={mine}
            disabled={mining}
            className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground disabled:opacity-50 flex-shrink-0"
          >
            <Wand2 className="w-3.5 h-3.5" /> {mining ? '挖掘中…' : '从历史挖掘'}
          </button>
        </div>
      </Card>

      {/* 待确认（pending>0 才显示）*/}
      {pending.length > 0 && (
        <Card icon={Sparkles} title={`待确认（${pending.length}）`}>
          {pending.map((h) => (
            <Row key={h.id}>
              <span className="flex-1 truncate text-sm">{h.word}</span>
              <div className="flex items-center gap-1 flex-shrink-0">
                <button
                  onClick={() => confirm(h.id)}
                  className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
                >
                  <Check className="w-3.5 h-3.5" /> 确认
                </button>
                <button
                  onClick={() => remove(h.id)}
                  className="rounded p-1 text-muted-foreground hover:text-red-500"
                  aria-label="丢弃"
                >
                  <Trash2 className="w-3.5 h-3.5" />
                </button>
              </div>
            </Row>
          ))}
        </Card>
      )}

      {/* 生效热词 —— 搜索 + 排序 + 卡片网格 */}
      <Card icon={BookMarked} title={`生效热词（${active.length}）`}>
        {!loaded ? (
          <p className="py-8 text-center text-sm text-muted-foreground">加载中…</p>
        ) : active.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-2 py-10 text-center">
            <div className="flex h-10 w-10 items-center justify-center rounded-full bg-muted text-muted-foreground">
              <Plus className="w-4 h-4" />
            </div>
            <p className="text-sm font-medium">还没有生效热词</p>
            <p className="text-xs text-muted-foreground">添加常被误识别的专名，识别后自动校正。</p>
          </div>
        ) : (
          <>
            {/* 搜索 + 排序 */}
            <div className="flex items-center gap-2 py-2.5 border-b border-border/40">
              <div className="relative flex-1 min-w-0">
                <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground/50 pointer-events-none" />
                <input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="搜索（拼音首字母 / 汉字）"
                  className="w-full bg-background border border-border rounded pl-7 pr-2.5 py-1.5 text-sm outline-none focus:border-voice/50"
                />
              </div>
              <select
                value={sort}
                onChange={(e) => setSort(e.target.value as 'time' | 'alpha' | 'hits')}
                className={cn(selectClass, 'flex-shrink-0')}
                aria-label="排序方式"
              >
                <option value="time">最近</option>
                <option value="alpha">字母</option>
                <option value="hits">命中度</option>
              </select>
            </div>
            {visible.length === 0 ? (
              <p className="py-8 text-center text-sm text-muted-foreground">无匹配热词</p>
            ) : (
              <div className="flex flex-wrap gap-2 py-2.5">
                {visible.map((h) => (
                  <div
                    key={h.id}
                    className="relative rounded-md border border-border bg-background px-3 py-2 pr-7 min-w-[112px] max-w-[200px] transition-colors hover:border-foreground/25"
                  >
                    {/* 右上角删除 */}
                    <button
                      onClick={() => remove(h.id)}
                      className="absolute top-1 right-1 rounded p-0.5 text-muted-foreground/60 hover:text-red-500"
                      aria-label={`删除热词 ${h.word}`}
                    >
                      <X className="w-3 h-3" />
                    </button>
                    {/* 词名 */}
                    <div className="text-sm truncate">{h.word}</div>
                    {/* meta：方式色点 + 命中数（>0 高亮 / =0 淡） */}
                    <div className="mt-1 flex items-center gap-2">
                      <SourceTag source={h.source} />
                      <span className={cn(
                        'font-mono text-[10px] tabular-nums',
                        h.hitCount > 0 ? 'text-voice' : 'text-muted-foreground/50',
                      )}>
                        {h.hitCount}
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </Card>
    </div>
  );
}
