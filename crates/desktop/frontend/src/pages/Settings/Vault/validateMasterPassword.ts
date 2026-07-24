/**
 * 主密码强度校验。
 *
 * 策略：长度 ≥ 12 位 + 必含 4 类字符（大写/小写/数字/符号）。
 *
 * 熵估算（V1 修正）：95 (可打印 ASCII) × 12 位 ≈ 79 bit 是理论上界（假设每位
 * 均匀随机选）。本策略只强制「4 类各含 1 个」+ 长度 12，最弱合法密码实际熵远低于
 * 此值。Argon2id (t=3, m=64MiB) 为弱密码兜底，抵抗 GPU 离线暴力。
 *
 * 不采用纯字符种类策略（NIST SP 800-63B 反对），但与项目既有文案
 * 「至少 12 位，含大小写数字符号」保持一致。
 *
 * 符号集：与密码生成器 SYMBOLS 常量保持一致（!@#$%^&*()-_=+[]{}<>?），
 * 外加中文用户常用的全角符号。
 */

export type MasterPasswordIssue =
    | "too_short"           // 长度不足
    | "missing_uppercase"   // 缺大写字母
    | "missing_lowercase"   // 缺小写字母
    | "missing_digit"       // 缺数字
    | "missing_symbol";     // 缺符号

export const MIN_MASTER_PASSWORD_LENGTH = 12;

/**
 * 标准符号集（ASCII 可打印，与密码生成器一致）。
 * 含中文用户常见的全角符号变体。
 */
const SYMBOL_CHARS = new Set<string>([
    // ASCII 符号（全集：所有非字母数字的 ASCII 可打印字符）
    // 与后端 validate.rs is_symbol 的 ASCII 段（is_ascii_graphic && !is_ascii_alphanumeric）一致
    ...Array.from("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"),
    // 中文用户常用全角符号（与后端 validate.rs is_symbol 的全角段逐字对齐）
    // V4 修复（2026-07-24）：¥ 改 ￥（U+00A5 → U+FFE5）对齐后端码点
    ...Array.from("！￥…（）—【】「」『』；：”“’‘，。、"),
]);

/**
 * 返回缺失的字符类别列表。空数组 = 通过。
 *
 * 调用方可根据返回的第一个 issue 给出精准的 UI 提示，
 * 或直接用 `validateMasterPassword(pwd).ok` 做布尔判断。
 */
export function findMasterPasswordIssues(password: string): MasterPasswordIssue[] {
    const issues: MasterPasswordIssue[] = [];

    if (password.length < MIN_MASTER_PASSWORD_LENGTH) {
        issues.push("too_short");
    }
    if (!/[A-Z]/.test(password)) {
        issues.push("missing_uppercase");
    }
    if (!/[a-z]/.test(password)) {
        issues.push("missing_lowercase");
    }
    if (!/\d/.test(password)) {
        issues.push("missing_digit");
    }
    if (![...password].some((c) => SYMBOL_CHARS.has(c))) {
        issues.push("missing_symbol");
    }

    return issues;
}

export interface MasterPasswordValidationResult {
    ok: boolean;
    issues: MasterPasswordIssue[];
}

export function validateMasterPassword(
    password: string,
): MasterPasswordValidationResult {
    const issues = findMasterPasswordIssues(password);
    return { ok: issues.length === 0, issues };
}
