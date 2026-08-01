#!/usr/bin/env bash
# 一次性迁移：dev DB schema 54→55（prompts 加 app_bundle_ids + inject_context）
#
# 背景：应用感知润色给 prompts 表加了两列。init_schema 对 0<v<CURRENT 一律 bail，
# 旧 dev DB（v54）需迁移才能继续跑核心测试 / 启动 app。
#
# 本脚本做最小化 ALTER（不丢其他表数据）：加列 + 给 3 条系统 prompt seed 填
# inject_context（app-casual=1，其余=0）+ 升 user_version 到 55。
#
# 幂等：列已存在时 ADD COLUMN 报错被忽略；user_version 已是 55 时 UPDATE no-op。
#
# 用法：./scripts/migrate-db-54-to-55.sh
set -euo pipefail

DB="${OCTOPUS_DB_PATH:-$HOME/.octopus/octopus.db}"

if [[ ! -f "$DB" ]]; then
  echo "✗ DB 不存在：$DB" >&2
  exit 1
fi

V=$(sqlite3 "$DB" "PRAGMA user_version;" | tr -dc '0-9')
echo "当前 schema version: $V"

if [[ "$V" -gt 54 ]]; then
  echo "✓ 已是 v (>=55)，无需迁移"
  exit 0
fi

if [[ "$V" -lt 54 ]]; then
  echo "✗ 版本 $V < 54，本脚本只支持 54→55。请先升级到 54 或清库重建。" >&2
  exit 1
fi

echo "迁移 54→55：prompts 加 app_bundle_ids + inject_context 列..."

# ALTER TABLE ADD COLUMN（已存在则忽略错误——幂等）
sqlite3 "$DB" "ALTER TABLE prompts ADD COLUMN app_bundle_ids TEXT NOT NULL DEFAULT '';" 2>/dev/null || echo "  app_bundle_ids 列已存在，跳过"
sqlite3 "$DB" "ALTER TABLE prompts ADD COLUMN inject_context INTEGER NOT NULL DEFAULT 0;" 2>/dev/null || echo "  inject_context 列已存在，跳过"

# 给系统 seed 填 inject_context（app-casual id=3 → 1，其余 → 0）
# content 字段是文件名引用，app-casual 的 dest_filename = "润色-口语化"
sqlite3 "$DB" "UPDATE prompts SET inject_context = CASE WHEN content = '润色-口语化' THEN 1 ELSE 0 END WHERE is_system = 1;"
echo "  系统 prompt inject_context 已填（app-casual=1，faithful/user-intent=0）"

# 升 user_version
sqlite3 "$DB" "PRAGMA user_version = 55;"
echo "✓ 迁移完成，user_version = $(sqlite3 "$DB" "PRAGMA user_version;")"
