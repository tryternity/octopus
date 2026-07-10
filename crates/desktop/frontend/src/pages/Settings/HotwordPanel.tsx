import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface Hotword {
  id: number;
  word: string;
  status: string;
  source: string;
  hitCount: number;
  createdAt: string;
}

interface Props {
  /** app_config.fuzzy_dialect（逗号分隔 token：f/h、hu/wu、n/l） */
  dialect: string;
  setVal: (key: string, value: string | number | boolean) => Promise<void>;
}

// 方言模糊选项——token 与后端 hotword::parse_dialect 对齐。
const DIALECT_OPTIONS: { tok: string; label: string }[] = [
  { tok: 'f/h', label: 'f/h 不分（浮 / 护）' },
  { tok: 'hu/wu', label: 'hu/wu 不分（黄 / 王）' },
  { tok: 'n/l', label: 'n/l 不分（刘 / 牛）' },
];

export function HotwordPanel({ dialect, setVal }: Props) {
  const [active, setActive] = useState<Hotword[]>([]);
  const [pending, setPending] = useState<Hotword[]>([]);
  const [input, setInput] = useState('');
  const [mining, setMining] = useState(false);

  async function refresh() {
    setActive(await invoke<Hotword[]>('list_hotwords', { status: 'active' }));
    setPending(await invoke<Hotword[]>('list_hotwords', { status: 'pending' }));
  }

  useEffect(() => {
    refresh();
  }, []);

  async function add() {
    const w = input.trim();
    if (!w) return;
    await invoke('add_hotword', { word: w });
    setInput('');
    await refresh();
  }

  async function confirm(id: number) {
    await invoke('confirm_pending_hotword', { id });
    await refresh();
  }

  async function remove(id: number) {
    await invoke('delete_hotword', { id });
    await refresh();
  }

  async function mine() {
    setMining(true);
    try {
      const n = await invoke<number>('mine_hotword_candidates');
      // eslint-disable-next-line no-alert
      alert(`挖掘完成，新增 ${n} 条候选`);
    } finally {
      setMining(false);
      await refresh();
    }
  }

  // 勾选/取消某方言组 → 重算逗号分隔串写回 app_config.fuzzy_dialect。
  function toggleDialect(tok: string) {
    const set = new Set(dialect.split(',').map((s) => s.trim()).filter(Boolean));
    if (set.has(tok)) {
      set.delete(tok);
    } else {
      set.add(tok);
    }
    void setVal('fuzzy_dialect', [...set].join(','));
  }

  const enabledTokens = new Set(dialect.split(',').map((s) => s.trim()));

  return (
    <div style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 16 }}>
      <section>
        <h3>方言模糊（按口音扩大热词召回）</h3>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {DIALECT_OPTIONS.map(({ tok, label }) => (
            <label key={tok} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <input
                type="checkbox"
                checked={enabledTokens.has(tok)}
                onChange={() => toggleDialect(tok)}
              />
              <span>{label}</span>
            </label>
          ))}
        </div>
        <small style={{ color: '#888' }}>
          勾选后热词纠错按对应方言模糊匹配（声母混淆互通）；不勾仅基础规则（平翘舌 + 前后鼻音）。
        </small>
      </section>

      <section>
        <h3>添加热词</h3>
        <div style={{ display: 'flex', gap: 8 }}>
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && add()}
            placeholder="人名 / 地名 / 术语 / 口头禅"
            style={{ flex: 1 }}
          />
          <button onClick={add}>添加</button>
          <button onClick={mine} disabled={mining}>
            {mining ? '挖掘中…' : '从历史挖掘'}
          </button>
        </div>
      </section>

      {pending.length > 0 && (
        <section>
          <h3>待确认（挖掘候选）</h3>
          <ul style={{ listStyle: 'none', padding: 0 }}>
            {pending.map((h) => (
              <li key={h.id} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <span>{h.word}</span>
                <button onClick={() => confirm(h.id)}>确认</button>
                <button onClick={() => remove(h.id)}>丢弃</button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section>
        <h3>生效热词（{active.length}）</h3>
        <ul style={{ listStyle: 'none', padding: 0 }}>
          {active.map((h) => (
            <li key={h.id} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <span>{h.word}</span>
              <small style={{ color: '#888' }}>
                {h.source === 'mined' ? '挖掘' : '手动'} · 命中 {h.hitCount}
              </small>
              <button onClick={() => remove(h.id)}>删除</button>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
