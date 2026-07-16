//! 统一搜索引擎：整合应用、菜单、Quicklinks、文件、书签。
//!
//! Task 4 重构：SearchEngine 持有 `Vec<Box<dyn SearchProvider>>`，
//! search() 用 `join_all` 并发调用各 Provider。Provider 子模块在 `providers/` 下，
//! Task 4 仅建 stub（search 返回空 vec），Task 5-9 逐步填实现。
//! 行为目标：与旧 search() 等价（所有非 ignore 测试通过）。

use std::sync::OnceLock;

use futures::future::join_all;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::Serialize;

use crate::frequency::FrequencyScorer;
use crate::provider::{SearchContext, SearchProvider};

/// 统一搜索结果。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub source: String,       // "app" | "file" | "menu" | "bookmark" | "quicklink" | "shell" | "calculator" | "url"
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>, // base64 data URI（应用图标等），None=用 source 默认图标
    pub action_type: String,  // "launch_app" | "open_file" | "menu" | "url" | "shell" | ...
    pub action_data: String,  // JSON
    pub score: i32,
}

/// 流式搜索的一批结果（emit 给前端）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBatch {
    pub run_id: String,
    pub results: Vec<SearchResult>, // 全局 top-10（已加权+排序+截断）
}

/// 全局搜索引擎（启动时初始化一次）。
///
/// `app_index` / `bookmarks` 用 `RwLock` 包裹——供 Provider 通过 `SearchContext` 只读访问，
/// 后台 mtime 轮询线程检测到应用目录变化时，可通过 `refresh_app_index` 替换内存索引，
/// 无需重启进程。搜索走读锁（无阻塞）。
pub struct SearchEngine {
    providers: Vec<Box<dyn SearchProvider>>,
    app_index: parking_lot::RwLock<crate::app_index::AppIndex>,
    bookmarks: parking_lot::RwLock<Vec<crate::bookmark::BookmarkEntry>>,
    frequency: FrequencyScorer,
}

/// 单次 search 返回的最大结果数。
const MAX_RESULTS: usize = 10;

static SEARCH_ENGINE: OnceLock<SearchEngine> = OnceLock::new();

/// 生产用默认 Provider 装配：7 个 Provider 全部启用（shell 另含 shell_commands +
/// shell_history 两个辅助模块，非独立 Provider）。
/// stub 阶段每个 Provider 的 search() 都返回空 vec；Task 5-9 填真实实现。
fn default_providers() -> Vec<Box<dyn SearchProvider>> {
    vec![
        Box::new(crate::providers::app::AppProvider),
        Box::new(crate::providers::file::FileProvider),
        Box::new(crate::providers::menu::MenuProvider),
        Box::new(crate::providers::bookmark::BookmarkProvider),
        Box::new(crate::providers::shell::ShellProvider::new()),
        Box::new(crate::providers::calculator::CalculatorProvider),
        Box::new(crate::providers::url::UrlProvider),
    ]
}

pub fn init_search_engine() {
    SEARCH_ENGINE.get_or_init(|| {
        let bookmarks = crate::bookmark::load_all_bookmarks();
        SearchEngine {
            providers: default_providers(),
            app_index: parking_lot::RwLock::new(crate::app_index::AppIndex::scan()),
            bookmarks: parking_lot::RwLock::new(bookmarks),
            frequency: FrequencyScorer::load(),
        }
    });
}

impl SearchEngine {
    /// 测试用构造函数——直接注入内存 app_index + bookmarks + providers，
    /// 不触达文件系统/DB。frequency 用空 HashMap。
    #[cfg(test)]
    fn new_for_test(
        apps: Vec<crate::app_index::AppEntry>,
        bookmarks: Vec<crate::bookmark::BookmarkEntry>,
        providers: Vec<Box<dyn SearchProvider>>,
    ) -> Self {
        SearchEngine {
            providers,
            app_index: parking_lot::RwLock::new(crate::app_index::AppIndex { apps }),
            bookmarks: parking_lot::RwLock::new(bookmarks),
            frequency: FrequencyScorer::with_test_data(std::collections::HashMap::new()),
        }
    }

    /// 测试用构造函数（注入非空 frequency）——供 streaming boost 回归测试使用。
    /// 与 `new_for_test` 同语义，但允许传入预构造的频次数据。
    #[cfg(test)]
    fn new_for_test_with_freq(
        apps: Vec<crate::app_index::AppEntry>,
        bookmarks: Vec<crate::bookmark::BookmarkEntry>,
        providers: Vec<Box<dyn SearchProvider>>,
        freqs: std::collections::HashMap<String, octopus_infra::db::FreqRow>,
    ) -> Self {
        SearchEngine {
            providers,
            app_index: parking_lot::RwLock::new(crate::app_index::AppIndex { apps }),
            bookmarks: parking_lot::RwLock::new(bookmarks),
            frequency: FrequencyScorer::with_test_data(freqs),
        }
    }

    /// 强制重扫应用索引：扫文件系统 + 写 DB 缓存 + 替换内存索引。
    /// 供后台 mtime 轮询线程（main.rs）和 reindex_apps 诊断命令复用。
    /// 返回扫描到的应用数。
    pub fn refresh_app_index(&self) -> usize {
        let new_index = crate::app_index::AppIndex::rescan();
        let n = new_index.apps.len();
        // 写锁仅持续替换瞬间——rescan() 的扫盘耗时发生在锁外
        *self.app_index.write() = new_index;
        n
    }

    /// 综合搜索（并发）。
    ///
    /// tab = "all" | "apps" | "files" | "shell" | "bookmarks" | "quick" | "files_bookmarks"。
    /// - "all"：所有 Provider 参与。
    /// - 其他 tab：仅 `provider.matches_tab(tab)` 为真的 Provider 参与。
    /// - "quick"：仅即时搜索（应用+菜单+Quicklinks+shell 模式），无文件/书签。
    /// - "files_bookmarks"：仅延迟搜索（文件+书签）。
    ///
    /// 所有匹配 Provider 通过 `join_all` 并发执行，结果合并、频次加权、按 score 降序排序、截断。
    pub async fn search(&self, query: &str, tab: &str) -> Vec<SearchResult> {
        if query.is_empty() {
            return Vec::new();
        }

        let ctx = SearchContext {
            app_index: &self.app_index,
            bookmarks: &self.bookmarks,
            frequency: &self.frequency,
        };

        // tab=="all" 时所有 Provider 参与；否则按 matches_tab 过滤。
        let active: Vec<_> = self
            .providers
            .iter()
            .filter(|p| tab == "all" || p.matches_tab(tab))
            .collect();

        // join_all 并发：所有 Provider 的 future 同时 poll，单 task 内并发（无 spawn）。
        let futures = active.into_iter().map(|p| p.search(query, &ctx));
        let batches = join_all(futures).await;

        // 合并 + 频次加权 + 排序 + 截断。
        let mut all: Vec<SearchResult> = batches.into_iter().flatten().collect();
        self.frequency.boost(&mut all, query);
        all.sort_by(|a, b| b.score.cmp(&a.score));
        all.truncate(MAX_RESULTS);
        all
    }

    /// 流式搜索：每个 Provider 完成立即 emit 一批全局 top-10。
    /// 用 FuturesUnordered 在单 task 内并发（不跨 spawn，避免 SearchContext 生命周期问题）。
    /// emit 为 FnMut 闭包：每完成一个 Provider 就用当前全局 top-10 调用一次。
    pub async fn search_streaming<F>(
        &self,
        query: &str,
        tab: &str,
        run_id: &str,
        mut emit: F,
    ) where
        F: FnMut(SearchBatch),
    {
        if query.is_empty() {
            emit(SearchBatch {
                run_id: run_id.to_string(),
                results: vec![],
            });
            return;
        }

        let ctx = SearchContext {
            app_index: &self.app_index,
            bookmarks: &self.bookmarks,
            frequency: &self.frequency,
        };

        let active: Vec<_> = self
            .providers
            .iter()
            .filter(|p| tab == "all" || p.matches_tab(tab))
            .collect();

        let mut futs = active
            .into_iter()
            .map(|p| p.search(query, &ctx))
            .collect::<FuturesUnordered<_>>();

        let mut collected: Vec<SearchResult> = Vec::new();
        while let Some(mut batch) = futs.next().await {
            // boost 只对新到的 batch 加权一次——若对 collected 整体 boost，
            // 先完成 Provider 的结果会被每一轮反复加分（boost 是加法性 `score += X`），
            // 违反「每次 emit 是全局 top-10（已正确加权）」不变量。
            self.frequency.boost(&mut batch, query);
            collected.extend(batch);
            collected.sort_by(|a, b| b.score.cmp(&a.score));
            collected.truncate(MAX_RESULTS);
            emit(SearchBatch {
                run_id: run_id.to_string(),
                results: collected.clone(),
            });
        }
    }
}

/// 获取全局搜索引擎（需先 init_search_engine）。
pub fn get_engine() -> Option<&'static SearchEngine> {
    SEARCH_ENGINE.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_index::AppEntry;
    use crate::provider::SearchProvider;
    use std::sync::Arc;

    /// Task 10：流式搜索——至少 emit 一次。
    #[tokio::test]
    async fn streaming_emits_at_least_once() {
        let engine = SearchEngine::new_for_test(
            vec![AppEntry {
                name: "TestApp".into(),
                path: "/Applications/TestApp.app".into(),
                aliases: vec![],
                icon: String::new(),
            }],
            vec![],
            test_providers(),
        );
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = count.clone();
        engine
            .search_streaming("test", "all", "run1", move |_batch| {
                count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .await;
        assert!(
            count.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "应至少 emit 一次"
        );
    }

    /// Task 10：空 query 时 emit 一次空 batch。
    #[tokio::test]
    async fn streaming_empty_query_emits_once_empty() {
        let engine = SearchEngine::new_for_test(vec![], vec![], test_providers());
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let results_len = Arc::new(std::sync::atomic::AtomicUsize::new(999));
        let (c1, r1) = (count.clone(), results_len.clone());
        engine
            .search_streaming("", "all", "run2", move |batch| {
                c1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                r1.store(batch.results.len(), std::sync::atomic::Ordering::SeqCst);
            })
            .await;
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(results_len.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// 测试用默认 Provider 装配（与生产 `default_providers()` 同构）。
    /// stub 阶段各 Provider search() 返回空；Task 5-9 填实现后这些测试逐个恢复。
    fn test_providers() -> Vec<Box<dyn SearchProvider>> {
        default_providers()
    }

    /// Task 10 review fix：streaming 不应反复对累积 collected 调 boost。
    /// boost 是加法性（`score += X`），若每轮对 collected 整体加权，先完成的
    /// Provider 结果会被后续轮次重复加分。回归断言：流式最后一次 emit 中每个结果
    /// 的分数 == 非流式 search 中对应结果的分数（boost 只加一次）。
    ///
    /// 触发 bug 需要「多轮 emit」——单 Provider 单轮不会暴露。故装配两个都匹配
    /// query、且各自结果都能被 frequency.boost 命中的 Provider，制造多 batch 场景。
    #[tokio::test]
    async fn streaming_boost_applied_once_not_per_round() {
        use async_trait::async_trait;
        use crate::provider::{SearchContext, SearchProvider};

        /// Mock Provider：固定产出一条 source=app 的结果，score_key=app|<path>。
        /// 与 AppProvider 同源（boost 不会跳过 app），但路径独立，便于隔离测试。
        struct MockAppProvider { path: &'static str, base: i32 }
        #[async_trait]
        impl SearchProvider for MockAppProvider {
            fn id(&self) -> &'static str { "app" }
            fn matches_tab(&self, _tab: &str) -> bool { true }
            async fn search(&self, _query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
                vec![SearchResult {
                    source: "app".into(),
                    title: format!("Mock-{}", self.path),
                    subtitle: String::new(),
                    icon: None,
                    action_type: "launch_app".into(),
                    action_data: serde_json::json!({ "path": self.path }).to_string(),
                    score: self.base,
                }]
            }
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut freqs = std::collections::HashMap::new();
        // 两个 score_key 都命中 frequency（hit_count=1，今天用过 → +3000）。
        // query="a" 也与 FreqRow.query 匹配 → 额外 +500。总 boost = 3500/结果。
        freqs.insert("app|/A.app".to_string(), octopus_infra::db::FreqRow {
            hit_count: 1, last_hit_ts: now, query: "a".into(),
        });
        freqs.insert("app|/B.app".to_string(), octopus_infra::db::FreqRow {
            hit_count: 1, last_hit_ts: now, query: "a".into(),
        });

        let engine = SearchEngine::new_for_test_with_freq(
            vec![],
            vec![],
            vec![
                Box::new(MockAppProvider { path: "/A.app", base: 1000 }),
                Box::new(MockAppProvider { path: "/B.app", base: 900 }),
            ],
            freqs,
        );

        // 收集最后一次 emit 的所有结果（按 path 建索引）。
        let last_snapshot: Arc<parking_lot::Mutex<Vec<SearchResult>>> =
            Arc::new(parking_lot::Mutex::new(vec![]));
        let snap_clone = last_snapshot.clone();
        engine
            .search_streaming("a", "all", "run1", move |batch| {
                *snap_clone.lock() = batch.results.clone();
            })
            .await;

        // 非流式 search（boost 只加一次，作为黄金参照）。
        let non_stream = engine.search("a", "all").await;

        let snap = last_snapshot.lock();
        assert_eq!(snap.len(), non_stream.len(),
            "streaming 最后一次 emit 的结果数应与非流式 search 一致");

        // 逐 path 比对 score：bug 表现为 streaming 中先完成 Provider 的结果
        // 被重复 boost（分数 > 非流式对应项）。
        let score_of = |results: &[SearchResult], path: &str| -> i32 {
            results.iter()
                .find(|r| r.action_data.contains(path))
                .map(|r| r.score)
                .unwrap_or(i32::MIN)
        };
        for path in &["A.app", "B.app"] {
            let s = score_of(&snap, path);
            let ns = score_of(&non_stream, path);
            assert_eq!(s, ns,
                "path={} streaming score {} != non-stream {}（boost 应只加一次）",
                path, s, ns);
        }
    }

    /// 切换到 in-memory DB，避免测试读 ~/.octopus/octopus.db。
    /// SearchEngine::search 在 tab=all/quick 时经 list_action_bar_items → with_db 触达 DB。
    /// 详见架构文档「测试数据库隔离」。
    static TEST_DB_SETUP: std::sync::Once = std::sync::Once::new();
    fn setup_test_db() {
        TEST_DB_SETUP.call_once(|| {
            octopus_infra::db::init_test_db();
        });
    }

    #[test]
    fn search_empty_returns_empty() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine::new_for_test(vec![], vec![], test_providers());
        let results = rt.block_on(engine.search("", "all"));
        assert!(results.is_empty());
    }

    /// Task 7（ShellProvider 实现）恢复——shell search 返回透传 + 补全 + 历史。
    #[test]
    fn shell_mode_prefix() {
        setup_test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine::new_for_test(vec![], vec![], test_providers());
        let results = rt.block_on(engine.search("> ls", "shell"));
        assert!(results.iter().any(|r| r.source == "shell" && r.action_type == "shell"));
    }

    #[test]
    fn quick_tab_excludes_files_and_bookmarks() {
        setup_test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine::new_for_test(vec![], vec![], test_providers());
        // quick tab 搜索 → 无文件/书签结果（tab 过滤保证 FileProvider/BookmarkProvider 不参与）
        let results = rt.block_on(engine.search("test", "quick"));
        assert!(results.iter().all(|r| r.source != "file" && r.source != "bookmark"));
    }

    #[test]
    fn files_bookmarks_tab_excludes_apps_and_menus() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine::new_for_test(
            vec![AppEntry { name: "TestApp".into(), path: "/Applications/TestApp.app".into(), aliases: vec![], icon: String::new() }],
            vec![],
            test_providers(),
        );
        let results = rt.block_on(engine.search("test", "files_bookmarks"));
        // files_bookmarks tab：AppProvider/MenuProvider matches_tab 返回 false → 不参与
        assert!(results.iter().all(|r| r.source != "app"));
        assert!(results.iter().all(|r| r.source != "menu"));
    }

    /// Task 7（ShellProvider 实现）恢复——quick tab 包含 shell 透传项。
    #[test]
    fn quick_tab_includes_shell_mode() {
        setup_test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine::new_for_test(vec![], vec![], test_providers());
        let results = rt.block_on(engine.search("> echo hi", "quick"));
        assert!(results.iter().any(|r| r.source == "shell" && r.action_type == "shell"));
    }

    #[test]
    fn all_tab_returns_combined_results() {
        setup_test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine::new_for_test(
            vec![AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into(), aliases: vec![], icon: String::new() }],
            vec![],
            test_providers(),
        );
        let results = rt.block_on(engine.search("chr", "all"));
        // all tab 应包含应用结果
        assert!(results.iter().any(|r| r.source == "app" && r.title == "Chrome"));
    }

    #[test]
    fn url_type_returned_as_quicklink_source() {
        setup_test_db();
        // URL 类型菜单项在搜索结果中 source 为 "quicklink"，action_type 为 "url"
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine::new_for_test(vec![], vec![], test_providers());
        // stub 阶段 search 返回空；本测试只验证不 panic。
        // Task 6（MenuProvider 实现）后此测试可加内容断言。
        let _results = rt.block_on(engine.search("test", "all"));
    }

    /// 回归 S1：refresh_app_index 替换内存索引后，search 读到新数据。
    /// 不触达文件系统——直接操作 RwLock 验证"写后读"语义。
    #[test]
    fn refresh_app_index_replaces_in_memory_index() {
        let engine = SearchEngine::new_for_test(
            vec![AppEntry { name: "OldApp".into(), path: "/Applications/OldApp.app".into(), aliases: vec![], icon: String::new() }],
            vec![],
            test_providers(),
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        // 初始：搜 old 能命中
        let r = rt.block_on(engine.search("old", "apps"));
        assert!(r.iter().any(|x| x.title == "OldApp"), "初始索引应有 OldApp");

        // 模拟 refresh：直接替换内存索引（绕过 rescan 的文件系统扫描）
        *engine.app_index.write() = crate::app_index::AppIndex {
            apps: vec![AppEntry { name: "NewApp".into(), path: "/Applications/NewApp.app".into(), aliases: vec![], icon: String::new() }],
        };
        // 替换后：搜 new 命中，搜 old 不命中
        let r = rt.block_on(engine.search("new", "apps"));
        assert!(r.iter().any(|x| x.title == "NewApp"), "refresh 后应能搜到 NewApp");
        let r = rt.block_on(engine.search("old", "apps"));
        assert!(r.iter().all(|x| x.title != "OldApp"), "refresh 后 OldApp 应已从索引移除");
    }

    /// 回归 S1：多线程并发读 search + 写 refresh，RwLock 不死锁不 panic。
    #[test]
    fn app_index_rwlock_concurrent_safe() {
        let engine = std::sync::Arc::new(SearchEngine::new_for_test(
            vec![AppEntry { name: "App".into(), path: "/Applications/App.app".into(), aliases: vec![], icon: String::new() }],
            vec![],
            test_providers(),
        ));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];

        // 读线程：持续 search
        for _ in 0..3 {
            let eng = engine.clone();
            let stop = stop.clone();
            handles.push(std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = rt.block_on(eng.search("app", "apps"));
                }
            }));
        }
        // 写线程：多次替换内存索引
        for i in 0..20 {
            *engine.app_index.write() = crate::app_index::AppIndex {
                apps: vec![AppEntry {
                    name: format!("App{}", i),
                    path: format!("/Applications/App{}.app", i),
                    aliases: vec![],
                    icon: String::new(),
                }],
            };
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        for h in handles {
            h.join().expect("读线程不应 panic");
        }
        // 最终索引应是最后一次写入
        let r = rt.block_on(engine.search("app19", "apps"));
        assert!(r.iter().any(|x| x.title == "App19"), "最终应为 App19");
    }
}
