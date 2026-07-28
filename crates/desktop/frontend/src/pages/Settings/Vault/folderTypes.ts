/**
 * Folder 相关共享类型（follow-up #6）。
 *
 * 与后端 `octopus_vault::storage::FolderDto` 对齐（snake_case，无 rename_all）。
 * FolderDto.name 已在后端解密——前端只见明文。
 */

export interface FolderDto {
  id: string; // UUID 字符串（2026-07-21 v44）
  name: string;
  sortOrder: number;
  createdAt: string;
  updatedAt: string;
}

/**
 * 当前选中的 sidebar 项。
 * - "all"：所有未删条目（不含回收站）
 * - "favorites"：收藏且未删
 * - "trash"：已软删
 * - string：指定 folderId（UUID 字符串，未删条目）
 */
export type FolderSelection = "all" | "favorites" | "trash" | string;

/**
 * Sidebar 各项的条目计数（用于角标）。
 * key 是 folderId（string UUID）或上述特殊 selection 字符串。
 */
export type FolderCounts = {
  all: number;
  favorites: number;
  trash: number;
  [folderId: string]: number;
};
