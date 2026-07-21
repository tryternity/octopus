/**
 * 密码生成器配置构造——纯函数，从组件状态组装出后端 payload。
 *
 * 最初是为独立浮窗 PasswordGenerator/index.tsx 抽离（便于单测）；
 * 浮窗已删除，生成器内嵌进 CipherEditor，本文件继续作为契约层复用。
 *
 * 锁定契约——Rust 端 octopus_vault::generator::GeneratorConfig 用
 * `#[serde(tag = "mode", rename_all = "camelCase")]` 内标签，4 种变体序列化为
 * `random` / `passphraseEn` / `passphraseZh` / `pin`。final-review C1 曾因
 * 前后端命名约定不一致导致反序列化失败，本文件 + 配套测试是回归门。
 *
 * 字段命名约定：模式标签（mode）走 camelCase，其余字段维持 snake_case
 * （与 Rust 端 #[serde(default)] 默认值一致），所有变体字段扁平在外层（内标签 adjacently
 * tagged 的扁平化形式，serde `tag = "mode"` 配合 `& Struct` 展开）。
 */

export type Mode = "random" | "passphraseEn" | "passphraseZh" | "pin";

export interface RandomConfig {
  length: number;
  uppercase: boolean;
  lowercase: boolean;
  numbers: boolean;
  symbols: boolean;
  avoid_ambiguous: boolean;
}

export interface PassphraseEnConfig {
  word_count: number;
  separator: string;
  capitalize: boolean;
  include_number: boolean;
}

export interface PassphraseZhConfig {
  word_count: number;
  separator: string;
  include_number: boolean;
  include_symbol: boolean;
}

export interface PinConfig {
  length: number;
}

/**
 * 后端 invoke payload——对应 GeneratorConfig 枚举（内标签 mode + 扁平字段）。
 * discriminated union 保证 mode 与字段集合强绑定，TS 编译期校验。
 */
export type GeneratorPayload =
  | ({ mode: "random" } & RandomConfig)
  | ({ mode: "passphraseEn" } & PassphraseEnConfig)
  | ({ mode: "passphraseZh" } & PassphraseZhConfig)
  | ({ mode: "pin" } & PinConfig);

/** 默认 Random 配置（与 Rust RandomConfig::default() 一致）。 */
export const DEFAULT_RANDOM: RandomConfig = {
  length: 16,
  uppercase: true,
  lowercase: true,
  numbers: true,
  symbols: false,
  avoid_ambiguous: true,
};

/** 默认 PassphraseEn 配置（与 Rust PassphraseEnConfig::default() 一致）。 */
export const DEFAULT_EN: PassphraseEnConfig = {
  word_count: 3,
  separator: "-",
  capitalize: true,
  include_number: true,
};

/** 默认 PassphraseZh 配置（与 Rust PassphraseZhConfig::default() 一致）。 */
export const DEFAULT_ZH: PassphraseZhConfig = {
  word_count: 4,
  separator: "",
  include_number: true,
  include_symbol: false,
};

/** 默认 Pin 配置（与 Rust PinConfig::default() 一致）。 */
export const DEFAULT_PIN: PinConfig = { length: 6 };

/**
 * 根据 mode + 4 套配置组装后端 payload。
 *
 * 行为：返回的对象始终带 `mode` 字段（值 = 入参 mode）+ 对应变体的全部字段（扁平）。
 * 与 Rust 端 `#[serde(tag = "mode")]` 反序列化期望严格对齐。
 *
 * 不修改入参，纯函数。
 */
export function buildPayload(
  mode: Mode,
  random: RandomConfig,
  en: PassphraseEnConfig,
  zh: PassphraseZhConfig,
  pin: PinConfig,
): GeneratorPayload {
  switch (mode) {
    case "random":
      return { mode: "random", ...random };
    case "passphraseEn":
      return { mode: "passphraseEn", ...en };
    case "passphraseZh":
      return { mode: "passphraseZh", ...zh };
    case "pin":
      return { mode: "pin", ...pin };
  }
}
