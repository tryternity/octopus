/**
 * Folder 相关共享类型（follow-up #6）。
 *
 * 与后端 `octopus_vault::storage::FolderDto` 对齐（snake_case，无 rename_all）。
 * FolderDto.name 已在后端解密——前端只见明文。
 */

export interface FolderDto {
  id: number;
  name: string;
  sort_order: number;
  created_at: string;
  updated_at: string;
}

/**
 * 当前选中的 sidebar 项。
 * - "all"：所有未删条目（不含回收站）
 * - "favorites"：收藏且未删
 * - "trash"：已软删
 * - number：指定 folder_id（未删条目）
 */
export type FolderSelection = "all" | "favorites" | "trash" | number;

/**
 * Sidebar 各项的条目计数（用于角标）。
 * key 是 folder_id（number）或上述特殊 selection 字符串。
 */
export type FolderCounts = {
  all: number;
  favorites: number;
  trash: number;
  [folderId: number]: number;
};
