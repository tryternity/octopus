#!/usr/bin/env node
/**
 * 词典分词精简脚本——用 jieba 把长短语拆成短词，去重输出。
 *
 * 用法：
 *   npm install @node-rs/jieba
 *   node segment_dict.mjs <input.txt> [output.txt] [--max-len 4]
 *
 * 输入格式：每行 `词` 或 `词\tDF值`（tab/空格分隔）。
 * 输出格式：每行 `词\tDF值`（按 DF 降序），纯数字 token 自动过滤。
 *
 * 策略：
 * - 原始词 ≤ max-len（默认 4）字 → 直接保留（本身是有意义的术语，不该拆）
 * - 原始词 > max-len 字 → jieba HMM 分词拆短，DF 平摊到各分词
 * - 只保留 2-max-len 字纯中文词（过滤单字/英文/标点）
 * - 去重（同词 DF 累加），按总 DF 降序输出
 */
import { Jieba } from "@node-rs/jieba";
import { readFileSync, writeFileSync } from "node:fs";

const args = process.argv.slice(2);
if (args.length < 1) {
  console.error("用法: node segment_dict.mjs <input.txt> [output.txt] [--max-len 4]");
  process.exit(1);
}
const inputPath = args[0];
const outputPath = args.find(a => !a.startsWith("--") && a !== inputPath) 
  ?? inputPath.replace(/\.txt$/, "_segmented.txt");
const maxLenIdx = args.indexOf("--max-len");
const maxLen = maxLenIdx >= 0 ? parseInt(args[maxLenIdx + 1], 10) : 4;

const jieba = new Jieba();
const content = readFileSync(inputPath, "utf-8");
const lines = content.split("\n").filter(l => l.trim());

const wordScores = new Map();
let kept = 0, split = 0;

function addWord(word, df) {
  const chars = [...word];
  if (chars.length < 2 || chars.length > maxLen) return false;
  if (!chars.every(c => c >= "\u4e00" && c <= "\u9fff")) return false; // 纯中文
  wordScores.set(word, (wordScores.get(word) ?? 0) + df);
  return true;
}

for (const line of lines) {
  const parts = line.trim().split(/\s+/);
  const origWord = parts[0];
  if (!origWord || /^\d+$/.test(origWord)) continue; // 纯数字跳过
  const df = parts.length >= 2 ? (parseInt(parts[1], 10) || 0) : 0;
  const charLen = [...origWord].length;

  if (charLen <= maxLen) {
    if (addWord(origWord, df)) kept++;
  } else {
    split++;
    const tokens = jieba.cut(origWord, true).map(t => t.trim()).filter(Boolean);
    const valid = tokens.filter(t => {
      const cs = [...t];
      return cs.length >= 2 && cs.length <= maxLen 
        && cs.every(c => c >= "\u4e00" && c <= "\u9fff");
    });
    const share = valid.length > 0 ? df / valid.length : 0;
    for (const w of valid) addWord(w, share);
  }
}

const sorted = [...wordScores.entries()].sort((a, b) => b[1] - a[1]);
const out = sorted.map(([w, s]) => `${w}\t${Math.round(s)}`).join("\n") + "\n";
writeFileSync(outputPath, out);

console.log(`输入: ${lines.length} 行 (短词保留 ${kept}, 长短语分词 ${split})`);
console.log(`输出: ${sorted.length} 去重词 → ${outputPath} (max-len=${maxLen})`);
