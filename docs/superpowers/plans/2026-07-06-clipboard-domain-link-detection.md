# 剪贴板无协议域名链接识别 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让剪贴板文本条目在无协议时也能识别为可点击链接——常用域名后缀（补 `https://`）与 localhost/IPv4+端口（补 `http://`），并消除两处重复的内联正则。

**Architecture:** 在 `types/clipboard.ts` 新增纯函数 `detectUrl(raw)`（含分号分隔的常用后缀常量 + 辅助校验），两处消费点（`ClipboardItem.tsx`、`ClipboardPanel.tsx`）替换原 `/^https?:\/\//` 内联判定与 `openUrl` 调用。纯前端改动，不动后端/数据库/`item_type`。

**Tech Stack:** TypeScript + React + Vite，测试用 vitest 4.1.9（`npm test` = `vitest run`），别名 `@/` → `src/`。

**约定：** 除非另注，所有 shell 命令在 `crates/desktop/frontend/` 目录下执行；文件路径相对仓库根。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/frontend/src/types/clipboard.ts` | 既有剪贴板类型 + 共享工具（`metaParts`/`typeAccent`）。新增链接识别：常量 `COMMON_DOMAIN_SUFFIXES`、`DetectUrlResult`、`detectUrl` 及私有辅助 | 修改（追加） |
| `crates/desktop/frontend/src/types/clipboard.test.ts` | `detectUrl` 纯函数单测（colocate，与 `caret.test.ts` 风格一致，相对路径 import） | 新建 |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | 剪贴板历史行：用 `detectUrl` 判定是否显示「打开链接」按钮及打开 URL | 修改 |
| `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | 设置页剪贴板面板：同上，外加元信息行的链接预览文字 | 修改 |

---

## Task 1: 实现 `detectUrl` 纯函数（TDD）

**Files:**
- Create: `crates/desktop/frontend/src/types/clipboard.test.ts`
- Modify: `crates/desktop/frontend/src/types/clipboard.ts`（文件末尾追加）

- [ ] **Step 1: 写失败测试**

创建 `crates/desktop/frontend/src/types/clipboard.test.ts`：

```ts
import { describe, it, expect } from "vitest";
import { detectUrl } from "./clipboard";

describe("detectUrl", () => {
  const cases: Array<{ input: string; isLink: boolean; url?: string; note?: string }> = [
    // 带协议：原样（含 trim）
    { input: "https://github.com/a", isLink: true, url: "https://github.com/a" },
    { input: "http://x.com", isLink: true, url: "http://x.com" },
    { input: "  https://a.com/p  ", isLink: true, url: "https://a.com/p" },
    // 路径 A：常用后缀域名 → 补 https://
    { input: "github.com/bingreeky/MemEvolve", isLink: true, url: "https://github.com/bingreeky/MemEvolve" },
    { input: "github.com", isLink: true, url: "https://github.com" },
    { input: "foo.com.cn/bar", isLink: true, url: "https://foo.com.cn/bar" },
    { input: "foo.cn", isLink: true, url: "https://foo.cn" },
    { input: "github.com:8080/x", isLink: true, url: "https://github.com:8080/x" },
    // 路径 B：localhost/IPv4 + 必带端口 → 补 http://
    { input: "localhost:3000", isLink: true, url: "http://localhost:3000" },
    { input: "localhost:3000/admin", isLink: true, url: "http://localhost:3000/admin" },
    { input: "127.0.0.1:8080", isLink: true, url: "http://127.0.0.1:8080" },
    { input: "192.168.1.10:80", isLink: true, url: "http://192.168.1.10:80" },
    { input: "0.0.0.0:5000", isLink: true, url: "http://0.0.0.0:5000" },
    { input: "localhost:1", isLink: true, url: "http://localhost:1" },           // 端口下界
    { input: "localhost:65535", isLink: true, url: "http://localhost:65535" },   // 端口上界
    // 否定
    { input: "", isLink: false },
    { input: "localhost", isLink: false, note: "无端口" },
    { input: "127.0.0.1", isLink: false, note: "无端口" },
    { input: "localhost:abc", isLink: false, note: "端口非数字" },
    { input: "localhost:0", isLink: false, note: "端口 0 非法" },
    { input: "localhost:65536", isLink: false, note: "端口超界" },
    { input: "256.1.1.1:80", isLink: false, note: "IPv4 段 >255" },
    { input: "file.txt", isLink: false, note: "后缀不在表" },
    { input: "main.rs", isLink: false },
    { input: "readme.md", isLink: false },
    { input: "v1.2.3", isLink: false },
    { input: "hello.world", isLink: false },
    { input: "看这个 github.com/foo", isLink: false, note: "含空格" },
    { input: "（github.com/foo）", isLink: false, note: "括号致 label 非法" },
  ];

  for (const c of cases) {
    it(`${c.isLink ? "链接" : "非链接"} ← ${JSON.stringify(c.input)}${c.note ? ` (${c.note})` : ""}`, () => {
      const r = detectUrl(c.input);
      expect(r.isLink).toBe(c.isLink);
      if (c.isLink) expect(r.url).toBe(c.url);
    });
  }
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd crates/desktop/frontend && npx vitest run src/types/clipboard.test.ts`
Expected: FAIL，报 `detectUrl is not defined`（或 import 失败）。

- [ ] **Step 3: 实现 `detectUrl`**

在 `crates/desktop/frontend/src/types/clipboard.ts` **文件末尾**追加（不改动既有内容）：

```ts
// ===== 无协议链接识别 =====

/**
 * 常用域名后缀（无协议链接识别用）。
 * 域名（小写）以其中任一后缀结尾、且后缀前至少还有一个 label，即判为公网链接（补 https://）。
 * 后缀自带前导「.」，dot 对齐，避免子串误命中（如 foocom ≠ .com）。
 * 追加新后缀直接加分号项即可，例如 ".dev" / ".io" / ".gov.cn"。
 */
export const COMMON_DOMAIN_SUFFIXES = ".com;.cn;.com.cn;.net;.org";
const COMMON_SUFFIX_LIST = COMMON_DOMAIN_SUFFIXES.split(";").filter(Boolean);

export interface DetectUrlResult {
  isLink: boolean;
  /** 打开用的完整 URL；无协议时已按规则补全 http(s):// */
  url: string;
}

/** 端口合法：1–65535 的数字。 */
function isPort(p: string): boolean {
  return /^\d{1,5}$/.test(p) && Number(p) >= 1 && Number(p) <= 65535;
}

/** 合法 IPv4：4 段点分，每段 0–255。 */
function isIPv4(h: string): boolean {
  const parts = h.split(".");
  if (parts.length !== 4) return false;
  return parts.every((s) => /^\d{1,3}$/.test(s) && Number(s) <= 255);
}

/** 域名 labels：≥2 段，每段 [A-Za-z0-9-]+ 且不以 - 开头/结尾。 */
function isDomainLabels(d: string): boolean {
  const parts = d.split(".");
  if (parts.length < 2) return false;
  return parts.every((s) => /^[A-Za-z0-9-]+$/.test(s) && !s.startsWith("-") && !s.endsWith("-"));
}

/**
 * 识别剪贴板文本是否为链接。
 * - 带协议（http(s)://）→ 原样
 * - localhost/IPv4 + 必带端口 → 补 http://
 * - 常用后缀域名 + 可选路径/端口 → 补 https://
 * - 句中片段（含空白）、纯 IP/localhost（无端口）、非常见后缀 → 非链接
 */
export function detectUrl(raw: string): DetectUrlResult {
  const s = raw.trim();
  if (!s) return { isLink: false, url: "" };
  if (/^https?:\/\//i.test(s)) return { isLink: true, url: s };
  if (/\s/.test(s)) return { isLink: false, url: "" };

  const hostSeg = s.split(/[/?#]/)[0];

  // 路径 B：localhost / IPv4 + 必带 :port → http://
  const portMatch = hostSeg.match(/:([^:/?#]+)$/);
  if (portMatch) {
    const port = portMatch[1];
    const hostname = hostSeg.slice(0, -portMatch[0].length); // 去掉 ":port"
    if (isPort(port) && (hostname.toLowerCase() === "localhost" || isIPv4(hostname))) {
      return { isLink: true, url: "http://" + s };
    }
  }

  // 路径 A：常用后缀域名 → https://
  const domainPart = hostSeg.split(":")[0];
  if (isDomainLabels(domainPart)) {
    const lower = domainPart.toLowerCase();
    if (COMMON_SUFFIX_LIST.some((suf) => lower.endsWith(suf) && lower.length > suf.length)) {
      return { isLink: true, url: "https://" + s };
    }
  }

  return { isLink: false, url: "" };
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd crates/desktop/frontend && npx vitest run src/types/clipboard.test.ts`
Expected: PASS（34 个用例全绿——含 review 阶段 commit c8398e4 补的 5 条边界：`Foo.COM` / `ftp://host` / `a.com.` / `localhost:8080a` / `1.2.3.4:5:6`）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/types/clipboard.ts crates/desktop/frontend/src/types/clipboard.test.ts
git commit -m "feat(clipboard): 新增 detectUrl 无协议域名/IP+端口链接识别"
```

---

## Task 2: 接入 `ClipboardItem.tsx`

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`（import 段第 8 行、判定第 125 行、打开第 215 行）

- [ ] **Step 1: 改 import，引入 `detectUrl`**

将第 8 行：

```ts
import { metaParts, typeAccent, imageMeta } from "@/types/clipboard";
```

改为：

```ts
import { metaParts, typeAccent, imageMeta, detectUrl } from "@/types/clipboard";
```

- [ ] **Step 2: 改判定逻辑（第 125 行）**

将：

```ts
  const isUrl = item.item_type === "text" && /^https?:\/\//i.test(item.content.trim());
```

改为：

```ts
  const link = item.item_type === "text" ? detectUrl(item.content) : null;
  const isUrl = !!link?.isLink;
```

- [ ] **Step 3: 改打开调用（第 215 行）**

将「打开链接」按钮里的：

```ts
              openUrl(item.content.trim()).catch(console.error);
```

改为：

```ts
              if (link) openUrl(link.url).catch(console.error);
```

（按钮仅在 `isUrl` 为真、即 `link?.isLink` 为真时渲染，故 `link` 此刻非空。）

- [ ] **Step 4: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc -b`
Expected: 无错误退出（exit 0）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx
git commit -m "refactor(clipboard): ClipboardItem 接入 detectUrl，支持无协议链接"
```

---

## Task 3: 接入 `ClipboardPanel.tsx` + 全量回归

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`（import 段第 7 行、判定第 357 行、打开第 461 行）

- [ ] **Step 1: 改 import，引入 `detectUrl`**

将第 7 行：

```ts
import { metaParts, typeAccent, imageMeta } from "@/types/clipboard";
```

改为：

```ts
import { metaParts, typeAccent, imageMeta, detectUrl } from "@/types/clipboard";
```

- [ ] **Step 2: 改判定逻辑（第 357 行）**

将：

```ts
  const isUrl = item.item_type === "text" && /^https?:\/\//i.test(item.content.trim());
```

改为：

```ts
  const link = item.item_type === "text" ? detectUrl(item.content) : null;
  const isUrl = !!link?.isLink;
```

（第 451 行的预览文字仍用 `isUrl` 控制显示、内容仍是 `item.content.trim()`，无需改动。）

- [ ] **Step 3: 改打开调用（第 461 行）**

将「打开链接」按钮里的：

```ts
            onClick={(e) => { e.stopPropagation(); openUrl(item.content.trim()).catch(console.error); }}
```

改为：

```ts
            onClick={(e) => { e.stopPropagation(); if (link) openUrl(link.url).catch(console.error); }}
```

- [ ] **Step 4: 类型检查 + 全量测试**

Run: `cd crates/desktop/frontend && npx tsc -b && npm test`
Expected: tsc 无错误；vitest 全绿（含既有 `caret.test.ts` 与新 `clipboard.test.ts`）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx
git commit -m "refactor(clipboard): ClipboardPanel 接入 detectUrl，消除最后一处重复正则"
```

---

## Task 4: 手动验证（可选，无 e2e 基建）

纯函数逻辑由 Task 1 单测锁定；此项用于确认真实渲染与 `openUrl` 打开行为。

- [ ] 启动桌面端，在剪贴板历史里分别复制并观察下列内容是否出现「打开链接」按钮、点击是否打开正确地址：
  - `github.com/bingreeky/MemEvolve` → 打开 `https://github.com/...`
  - `localhost:3000` → 打开 `http://localhost:3000`
  - `127.0.0.1:8080/admin` → 打开 `http://127.0.0.1:8080/admin`
  - `file.txt` → **不**出现按钮
  - `localhost`（无端口）→ **不**出现按钮
- [ ] 同样在「设置 → 剪贴板」面板核对一遍（两处消费点一致性）。
