import { describe, it, expect } from "vitest";
import {
  buildPayload,
  DEFAULT_RANDOM,
  DEFAULT_EN,
  DEFAULT_ZH,
  DEFAULT_PIN,
  type RandomConfig,
  type PassphraseEnConfig,
  type PassphraseZhConfig,
  type PinConfig,
} from "./buildConfig";

/**
 * buildPayload 契约测试——锁定与 Rust GeneratorConfig 的 serde 约定。
 *
 * 关键不变量（对应 final-review C1 修复）：
 *   - mode 字段值精确匹配枚举变体标签（camelCase：passphraseEn / passphraseZh；
 *     小写：random / pin）
 *   - 字段名维持 snake_case（与 Rust 端 pub 字段一致，serde 默认 snake_case 序列化）
 *   - 各 mode 仅携带该变体的字段（不多不少）
 *   - 端到端 payload 字符串可被 Rust `serde_json::from_str::<GeneratorConfig>` 解析
 */

describe("buildPayload — mode 标签", () => {
  it("random → mode: 'random'", () => {
    const p = buildPayload("random", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    expect(p.mode).toBe("random");
  });
  it("passphraseEn → mode: 'passphraseEn'（camelCase，非 passphrase_en）", () => {
    const p = buildPayload("passphraseEn", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    expect(p.mode).toBe("passphraseEn");
  });
  it("passphraseZh → mode: 'passphraseZh'（camelCase，非 passphrase_zh）", () => {
    const p = buildPayload("passphraseZh", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    expect(p.mode).toBe("passphraseZh");
  });
  it("pin → mode: 'pin'", () => {
    const p = buildPayload("pin", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    expect(p.mode).toBe("pin");
  });
});

describe("buildPayload — 字段集合按 mode 分发", () => {
  it("random 模式携带 6 个字段（length + 5 个布尔）", () => {
    const p = buildPayload("random", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    // discriminated union 在 mode='random' 分支下应识别为 RandomConfig
    if (p.mode !== "random") throw new Error("expected random");
    expect(p).toEqual({
      mode: "random",
      length: 16,
      uppercase: true,
      lowercase: true,
      numbers: true,
      symbols: false,
      avoid_ambiguous: true,
    });
    // 反向校验：不应混入 passphrase / pin 字段
    expect("word_count" in p).toBe(false);
    expect("separator" in p).toBe(false);
    expect("capitalize" in p).toBe(false);
  });

  it("passphraseEn 模式携带 4 个字段（word_count + separator + capitalize + include_number）", () => {
    const p = buildPayload("passphraseEn", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    if (p.mode !== "passphraseEn") throw new Error("expected passphraseEn");
    expect(p).toEqual({
      mode: "passphraseEn",
      word_count: 3,
      separator: "-",
      capitalize: true,
      include_number: true,
    });
    expect("length" in p).toBe(false);
    expect("include_symbol" in p).toBe(false);
  });

  it("passphraseZh 模式携带 4 个字段（word_count + separator + include_number + include_symbol）", () => {
    const p = buildPayload("passphraseZh", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    if (p.mode !== "passphraseZh") throw new Error("expected passphraseZh");
    expect(p).toEqual({
      mode: "passphraseZh",
      word_count: 4,
      separator: "",
      include_number: true,
      include_symbol: false,
    });
    expect("capitalize" in p).toBe(false);
    expect("length" in p).toBe(false);
  });

  it("pin 模式仅携带 length 字段", () => {
    const p = buildPayload("pin", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    if (p.mode !== "pin") throw new Error("expected pin");
    expect(p).toEqual({ mode: "pin", length: 6 });
    expect("word_count" in p).toBe(false);
  });
});

describe("buildPayload — 用户改值后回写", () => {
  // 模拟用户在 UI 调整配置后的真实场景：随机模式 length=24 + 关闭 lowercase。
  it("random 反映用户自定义配置（length=24, lowercase=false）", () => {
    const custom: RandomConfig = {
      ...DEFAULT_RANDOM,
      length: 24,
      lowercase: false,
    };
    const p = buildPayload("random", custom, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    if (p.mode !== "random") throw new Error("expected random");
    expect(p.length).toBe(24);
    expect(p.lowercase).toBe(false);
    expect(p.uppercase).toBe(true); // 未改的字段保持
  });

  it("passphraseZh 反映自定义（separator='-'，include_symbol=true）", () => {
    const custom: PassphraseZhConfig = {
      ...DEFAULT_ZH,
      separator: "-",
      include_symbol: true,
    };
    const p = buildPayload("passphraseZh", DEFAULT_RANDOM, DEFAULT_EN, custom, DEFAULT_PIN);
    if (p.mode !== "passphraseZh") throw new Error("expected passphraseZh");
    expect(p.separator).toBe("-");
    expect(p.include_symbol).toBe(true);
    expect(p.word_count).toBe(4);
  });

  it("pin 反映 length=8", () => {
    const custom: PinConfig = { length: 8 };
    const p = buildPayload("pin", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, custom);
    if (p.mode !== "pin") throw new Error("expected pin");
    expect(p.length).toBe(8);
  });

  it("passphraseEn 反映 word_count=5", () => {
    const custom: PassphraseEnConfig = { ...DEFAULT_EN, word_count: 5 };
    const p = buildPayload("passphraseEn", DEFAULT_RANDOM, custom, DEFAULT_ZH, DEFAULT_PIN);
    if (p.mode !== "passphraseEn") throw new Error("expected passphraseEn");
    expect(p.word_count).toBe(5);
    expect(p.capitalize).toBe(true);
  });
});

describe("buildPayload — JSON 序列化兼容 Rust 反序列化（回归门 for C1）", () => {
  // 直接验证 JSON.stringify 输出可被 Rust `serde_json::from_str::<GeneratorConfig>` 接受。
  // 关键检查点：mode 标签存在且拼写正确，字段名 snake_case。
  // Rust 端 generator/mod.rs::test_generator_config_serde_modes 做对偶测试，
  // 此处的回归价值在于：捕获前端误把字段名改成 camelCase 的回归。
  it("passphraseZh 序列化后含 mode + snake_case 字段", () => {
    const p = buildPayload("passphraseZh", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    const json = JSON.stringify(p);
    expect(json).toContain('"mode":"passphraseZh"');
    expect(json).toContain('"word_count":4');
    expect(json).toContain('"include_number":true');
    expect(json).toContain('"include_symbol":false');
    // 不应出现 camelCase（回归门：若误写 includeNumber 就会被发现）
    expect(json).not.toContain('includeNumber');
    expect(json).not.toContain('includeSymbol');
    expect(json).not.toContain('wordCount');
  });

  it("passphraseEn 序列化后含 mode + snake_case 字段", () => {
    const p = buildPayload("passphraseEn", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    const json = JSON.stringify(p);
    expect(json).toContain('"mode":"passphraseEn"');
    expect(json).toContain('"word_count":3');
    expect(json).toContain('"include_number":true');
  });

  it("random 序列化后含 mode + 全部 6 字段", () => {
    const p = buildPayload("random", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    const json = JSON.stringify(p);
    expect(json).toContain('"mode":"random"');
    expect(json).toContain('"avoid_ambiguous":true');
    expect(json).not.toContain('avoidAmbiguous');
  });
});

describe("buildPayload — 纯函数性", () => {
  // buildPayload 不应改入参（避免 React state 突变）。
  it("不修改入参（random 配置对象）", () => {
    const before: RandomConfig = { ...DEFAULT_RANDOM };
    buildPayload("random", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    expect(DEFAULT_RANDOM).toEqual(before);
  });

  it("不修改入参（passphraseZh 配置对象）", () => {
    const before: PassphraseZhConfig = { ...DEFAULT_ZH };
    buildPayload("passphraseZh", DEFAULT_RANDOM, DEFAULT_EN, DEFAULT_ZH, DEFAULT_PIN);
    expect(DEFAULT_ZH).toEqual(before);
  });
});
