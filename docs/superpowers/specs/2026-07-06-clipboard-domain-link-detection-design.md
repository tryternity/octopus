# 剪贴板无协议域名链接识别 — 设计

- 日期：2026-07-06
- 分支：feature-0706
- 范围：桌面端前端（`crates/desktop/frontend`）

## 1. 背景与现状

剪贴板历史里的「文本」条目，若内容是 URL，前端会额外渲染一个「打开链接」按钮，点击用
`@tauri-apps/plugin-opener` 的 `openUrl` 打开；设置页的剪贴板面板还会在元信息行显示链接预览文字。

当前判定写死为「以 `http://` / `https://` 开头」的内联正则，且**同一逻辑在两处重复**：

- `pages/Clipboard/ClipboardItem.tsx:125`
- `pages/Settings/ClipboardPanel.tsx:357`

均为：

```ts
const isUrl = item.item_type === "text" && /^https?:\/\//i.test(item.content.trim());
```

打开调用：

```ts
openUrl(item.content.trim()).catch(console.error);
```

### 问题

很多场景复制的链接**不带协议前缀**，例如 `github.com/bingreeky/MemEvolve`、`zhihu.com/question/123`，
以及本地开发地址 `localhost:3000`、`127.0.0.1:8080`。当前规则识别不到，按钮不出现。

## 2. 目标 / 非目标

**目标**

- 无协议的「常见域名 + 可选路径」识别为链接（如 `github.com/foo`、`foo.com.cn/bar`）。
- 无协议的「localhost / IPv4 + `:port`」识别为链接（如 `localhost:3000`、`192.168.1.10:80`）。
- 消除两处重复，收敛为单一共享判定函数。
- 误判可控：`file.txt` / `main.rs` / `v1.2.3` / 句中片段不被误判。

**非目标**

- 不改后端、不改数据库 schema、不改 `item_type`（链接始终是 `text` 类型，仅前端渲染差异）。
- 不做 IPv6（`[::1]:3000`）。
- 不做邮件 `mailto:`、不带端口的纯 IP / 纯 `localhost`。
- 不做「链接混在整段文本里」的提取——判定对象是整条 `content`，夹在句中（含空白）不算。

## 3. 设计

### 3.1 共享 helper：`detectUrl`

放在 `types/clipboard.ts`（两处已共享此文件的 `metaParts` / `typeAccent`，零新增 import）。

签名：

```ts
export interface DetectUrlResult {
  isLink: boolean;
  /** 打开用的完整 URL；无协议时已按规则补全 http(s):// */
  url: string;
}
export function detectUrl(raw: string): DetectUrlResult;
```

调用点改造：

```ts
const link = detectUrl(item.content);
// 显示按钮：link.isLink
// 打开：openUrl(link.url)
// ClipboardPanel 预览文字：link.isLink
```

### 3.2 常用域名后缀常量

分号分隔字符串，便于手动追加：

```ts
/**
 * 无协议链接识别用的常用域名后缀。
 * 域名（小写）以其中任一后缀结尾、且后缀前至少还有一个 label，即判为公网链接（补 https://）。
 * 后缀自带前导「.」，天然 dot 对齐，避免子串误命中（如 foocom ≠ .com）。
 * 追加新后缀直接加分号项即可，例如 ".dev" / ".io" / ".gov.cn"。
 */
const COMMON_DOMAIN_SUFFIXES = ".com;.cn;.com.cn;.net;.org";
const COMMON_SUFFIX_LIST = COMMON_DOMAIN_SUFFIXES.split(";").filter(Boolean);
```

> 说明：`.cn` 单级后缀已覆盖 `*.com.cn`（`foo.com.cn` 末段就是 `cn`）。这里仍显式列出 `.com.cn`
> 以贴合「按后缀表自描述」的心智模型，两者任一命中即可。

### 3.3 判定算法

伪代码（`s = raw.trim()`）：

```
1. 若 s 为空 → 否
2. 若 s 匹配 ^[a-z][a-z0-9+.-]*:// （带协议）→ { isLink: true, url: s }   # 保留现有 http(s) 行为
3. 若 s 含任意空白字符 → 否                                              # 句中片段不算
4. hostSeg = s.split(/[/?#]/)[0]                                         # 第一个 / ? # 之前
5. 路径 B（本地服务地址，补 http://）：
     取 hostSeg 末尾的 :port：m = hostSeg.match(/:([^:/?#]+)$/)
     若 m 且 isPort(m[1]) 且 hostname = hostSeg 去掉该 :port 后满足：
         hostname.toLowerCase() === "localhost"  或  isIPv4(hostname)
     → { isLink: true, url: "http://" + s }
6. 路径 A（公网域名，补 https://）：
     domainPart = hostSeg.split(":")[0]                                  # 去端口
     若 isDomainLabels(domainPart)：
         lower = domainPart.toLowerCase()
         若 COMMON_SUFFIX_LIST 中存在 suf 使 lower.endsWith(suf) 且 lower.length > suf.length：
         → { isLink: true, url: "https://" + s }
7. 都不命中 → 否
```

辅助校验：

```
isPort(p)        = /^\d{1,5}$/.test(p) 且 1 ≤ Number(p) ≤ 65535
isIPv4(h)        = h 拆「.」恰 4 段，每段 /^\d{1,3}$/ 且 0..255
isDomainLabels(d)= d 拆「.」≥ 2 段，每段 /^[A-Za-z0-9-]+$/ 且不以「-」开头/结尾
```

**执行顺序**：B 先于 A。
- `localhost:3000` → 走 B，补 `http://`
- `github.com:8080/x` → B 不命中（hostname 非 localhost/IPv4）→ 落到 A，补 `https://`

**协议补全规则**：

- 带协议：原样
- 路径 B（本地服务）：`http://`（本地服务通常无 TLS）
- 路径 A（公网域名）：`https://`

### 3.4 文件位置

常量 + `detectUrl` + 辅助函数全部放 `types/clipboard.ts`，与既有 `metaParts` / `typeAccent` 同级。

### 3.5 改动面（3 文件）

| 文件 | 改动 |
|---|---|
| `types/clipboard.ts` | 新增常量 + `detectUrl` + 辅助校验函数 |
| `pages/Clipboard/ClipboardItem.tsx` | 第 125 行 `isUrl` 改用 `detectUrl(item.content).isLink`；第 215 行 `openUrl(item.content.trim())` 改 `openUrl(link.url)` |
| `pages/Settings/ClipboardPanel.tsx` | 第 357 行同上；第 451 行预览文字 `isUrl` 同上；第 461 行 `openUrl` 同上 |

## 4. 边界用例

| 输入 | 判定 | 打开为 / 依据 |
|---|---|---|
| `https://github.com/a` | ✓ | 原样（带协议） |
| `http://127.0.0.1:8080` | ✓ | 原样（带协议） |
| `github.com/bingreeky/MemEvolve` | ✓ | `https://…`（后缀 `.com`） |
| `github.com`（裸域名） | ✓ | `https://github.com` |
| `foo.com.cn/bar` | ✓ | 后缀 `.cn` / `.com.cn` |
| `foo.cn` | ✓ | 后缀 `.cn` |
| `bar.io`（若常量加了 `.io`） | ✓ | 后缀 `.io` |
| `github.com:8080/x` | ✓ | 走 A，`https://…` |
| `localhost:3000` | ✓ | `http://localhost:3000` |
| `localhost:3000/admin` | ✓ | B + 带路径 |
| `127.0.0.1:8080` | ✓ | `http://127.0.0.1:8080` |
| `192.168.1.10:80` | ✓ | 内网 IPv4 + 端口 |
| `0.0.0.0:5000` | ✓ | 合法 IPv4 |
| `localhost` | ✗ | 无端口 |
| `127.0.0.1` | ✗ | 无端口（路径 A 后缀也不匹配） |
| `localhost:abc` | ✗ | 端口非数字 |
| `999.999.999.999:80` | ✗ | 非法 IPv4 |
| `file.txt` / `main.rs` / `readme.md` | ✗ | 后缀不在表 |
| `v1.2.3` / `192.168.1.1`（无端口） | ✗ | 末段数字，后缀不匹配 |
| `hello.world` | ✗ | `.world` 不在表（保守） |
| `看这个 github.com/foo` | ✗ | 含空白 |
| `（github.com/foo）` | ✗ | 中文括号致 label 非法 |
| `[::1]:3000`（IPv6） | ✗ | IPv6 不支持（非目标） |

## 5. 测试计划

`detectUrl` 为纯函数，按项目既有前端测试框架补单测，覆盖第 4 节全部用例（每条至少一例，
`.io` 类用「常量含此后缀」的变体覆盖）。另对辅助函数 `isIPv4` / `isPort` / `isDomainLabels`
各加针对性用例（边界端口 0 / 65535 / 65536、IPv4 段 0 / 255 / 256、label 首尾 `-`）。

## 6. 风险与权衡

- **后缀表保守**：初始仅 5 项，长尾 TLD（`.world`/`.site`/`.online` 等）不在表，会漏判真实但少见的站点；
  代价是零误判普通词。用户可按需追加。
- **本地服务补 http**：少数本地 https 服务（如 `localhost:8443` 走 TLS）会被补成 `http://` 而连接失败。
  这类属少数，且用户可手动改协议；不为此引入协议探测。
- **无协议 + 端口的公网域名**走 https（如 `github.com:8080`），与本地走 http 通过 hostname 类型区分，符合直觉。
