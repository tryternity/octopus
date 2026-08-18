import { describe, expect, it } from "vitest";
import { ACTION_TYPES, TYPE_META, deriveAccepts } from "./constants";

describe("markdown action type", () => {
  it("markdown 在 ACTION_TYPES 中", () => {
    expect(ACTION_TYPES.some((t) => t.value === "markdown")).toBe(true);
  });

  it("markdown 有 TYPE_META", () => {
    expect(TYPE_META.markdown).toBeDefined();
    expect(TYPE_META.markdown.label).toBe("MD");
  });

  it("deriveAccepts: markdown → any，explicit 优先", () => {
    expect(deriveAccepts("markdown")).toBe("any");
    expect(deriveAccepts("markdown", "text")).toBe("text");
    expect(deriveAccepts(undefined)).toBe("text");
  });
});
