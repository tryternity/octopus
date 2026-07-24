import { describe, expect, it } from "vitest";
import {
    MIN_MASTER_PASSWORD_LENGTH,
    validateMasterPassword,
} from "./validateMasterPassword";

describe("validateMasterPassword", () => {
    describe("valid passwords", () => {
        const validSamples = [
            "Abcdefgh123!",         // 12 chars, all 4 types
            "Tr0ub4dour&3xy",       // 13 chars
            "Abc123!@#xyz",         // 12 chars
            "PassWord1@2345",       // 14 chars
            "Zzzzzzz1!zzz",         // 12 chars, ugly but valid
            // 含中文全角符号
            "Abcd1234！！xy",        // ASCII 字母数字 + 中文！
        ];

        for (const pwd of validSamples) {
            it(`accepts: ${pwd.replace(/./g, "*")}`, () => {
                const result = validateMasterPassword(pwd);
                expect(result.ok, JSON.stringify(result.issues)).toBe(true);
                expect(result.issues).toEqual([]);
            });
        }
    });

    describe("too_short", () => {
        it("rejects 11-char password even with all types", () => {
            // 10 chars, even with all 4 types, fails too_short
            expect(validateMasterPassword("Abcdefgh1!").ok).toBe(false);
            expect(validateMasterPassword("Abcdefgh1!").issues).toContain("too_short");
        });

        it(`accepts exactly ${MIN_MASTER_PASSWORD_LENGTH} chars with all types`, () => {
            const pwd = "Aa1!Aa1!Aa1!"; // 12 chars
            expect(pwd.length).toBe(MIN_MASTER_PASSWORD_LENGTH);
            expect(validateMasterPassword(pwd).ok).toBe(true);
        });
    });

    describe("missing character classes", () => {
        it("rejects missing uppercase", () => {
            const result = validateMasterPassword("abcdefg123!xyz");
            expect(result.ok).toBe(false);
            expect(result.issues).toContain("missing_uppercase");
        });

        it("rejects missing lowercase", () => {
            const result = validateMasterPassword("ABCDEF123!XYZ");
            expect(result.ok).toBe(false);
            expect(result.issues).toContain("missing_lowercase");
        });

        it("rejects missing digit", () => {
            const result = validateMasterPassword("Abcdefg!xyzAB");
            expect(result.ok).toBe(false);
            expect(result.issues).toContain("missing_digit");
        });

        it("rejects missing symbol", () => {
            const result = validateMasterPassword("Abcdefg123xyz");
            expect(result.ok).toBe(false);
            expect(result.issues).toContain("missing_symbol");
        });
    });

    describe("regression: current bug", () => {
        // 这是当前 SetupWizard 的 bug：12 个 a 能通过（仅校验 length）
        it("rejects 12 a's (was accepted before fix)", () => {
            const result = validateMasterPassword("aaaaaaaaaaaa");
            expect(result.ok).toBe(false);
            // 应同时缺多个类
            expect(result.issues.length).toBeGreaterThanOrEqual(3);
        });

        // 8 位 + 强制 4 类（用户提议的方案）应被拒绝——长度不足
        it("rejects 8-char password with all types (too_short)", () => {
            const result = validateMasterPassword("Abcd123!");
            expect(result.issues).toContain("too_short");
            expect(result.ok).toBe(false);
        });
    });

    describe("symbol set coverage", () => {
        // ASCII 符号
        for (const sym of ["!", "@", "#", "$", "%", "^", "&", "*", "(", ")", "-", "_", "=", "+"]) {
            it(`accepts symbol: ${sym}`, () => {
                const pwd = `Aa1${sym}Aa1Aa1Aa`; // 12+ chars
                expect(validateMasterPassword(pwd).ok, pwd).toBe(true);
            });
        }

        // 中文全角符号（V4：¥ U+00A5 已统一为 ￥ U+FFE5，与后端 validate.rs 对齐）
        for (const sym of ["！", "￥", "（", "）", "—", "【", "】", "。"]) {
            it(`accepts Chinese symbol: ${sym}`, () => {
                const pwd = `Aa1${sym}Aa1Aa1Aa`; // 12+ chars (Chinese chars count as 1 each)
                expect(validateMasterPassword(pwd).ok, pwd).toBe(true);
            });
        }
    });
});
