// 模型管理页（设置窗口页面 3）。独立文件——与 settings-ui2 分支在 index.html 上的改动隔离。
// 由 index.html 的 <script src="models.js"> 加载；switchPage('models') 调 initModelsPage()。
// 包在 IIFE 里：避免与 index.html 内联脚本的顶层 const（invoke/listen）重声明冲突。
(function () {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  let progressUnlisten = null;
  let fileUnlisten = null;
  let currentRepo = null; // 同一时刻只允许一个下载（v1 串行）

  const fmtBytes = (n) => {
    if (n == null) return '?';
    if (n < 1024) return n + ' B';
    if (n < 1048576) return (n / 1024).toFixed(1) + ' KB';
    if (n < 1073741824) return (n / 1048576).toFixed(1) + ' MB';
    return (n / 1073741824).toFixed(2) + ' GB';
  };

  const escapeHtml = (s) =>
    String(s).replace(/[&<>"']/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    }[c]));

  const toast = (msg) => {
    if (typeof window.showToast === 'function') window.showToast(msg);
    else console.log('[models]', msg);
  };

  function injectStyles() {
    if (document.getElementById('models-page-styles')) return;
    const style = document.createElement('style');
    style.id = 'models-page-styles';
    style.textContent = `
      .model-item { display: flex; align-items: center; justify-content: space-between;
        padding: 10px 0; border-bottom: 1px solid var(--border); gap: 12px; }
      .model-item:last-child { border-bottom: none; }
      .model-info { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
      .model-name { font-size: 14px; font-weight: 500; }
      .model-cat { font-size: 11px; color: var(--text-secondary); background: var(--sidebar-bg);
        padding: 1px 6px; border-radius: 4px; margin-left: 6px; font-weight: 400; }
      .model-desc { font-size: 12px; color: var(--text-secondary); }
      .model-repo { font-size: 11px; color: var(--text-secondary); word-break: break-all; }
      .model-action { flex-shrink: 0; display: flex; flex-direction: column; align-items: flex-end;
        gap: 4px; min-width: 170px; }
      .btn-download { padding: 6px 16px; border: 1px solid var(--primary); border-radius: 6px;
        background: var(--primary); color: white; font-size: 13px; cursor: pointer;
        transition: opacity 0.15s; }
      .btn-download:hover { opacity: 0.85; }
      .btn-download:disabled { opacity: 0.4; cursor: not-allowed; }
      .model-downloaded { font-size: 13px; color: var(--toggle-on); }
      .progress-wrap { width: 100%; }
      .progress-file { font-size: 11px; color: var(--text-secondary); margin-bottom: 3px;
        word-break: break-all; }
      .progress-bar { height: 6px; background: var(--border); border-radius: 3px; overflow: hidden; }
      .progress-fill { height: 100%; background: var(--primary); width: 0%; transition: width 0.2s; }
      .progress-text { font-size: 11px; color: var(--text-secondary); margin-top: 3px; }
      .progress-failed { font-size: 11px; color: #ef4444; margin-top: 3px; word-break: break-all; }
    `;
    document.head.appendChild(style);
  }

  async function renderModels() {
    const listEl = document.getElementById('models-list');
    if (!listEl) return;
    listEl.innerHTML = '<div style="color: var(--text-secondary); padding: 12px 0;">加载中...</div>';
    try {
      const models = await invoke('list_downloadable_models');
      if (!models.length) {
        listEl.innerHTML = '<div style="color: var(--text-secondary); padding: 12px 0;">无可下载模型</div>';
        return;
      }
      listEl.innerHTML = models.map((m) => `
        <div class="model-item">
          <div class="model-info">
            <span class="model-name">${escapeHtml(m.name)}<span class="model-cat">${escapeHtml(m.category)}</span></span>
            <span class="model-desc">${escapeHtml(m.description)}</span>
            <span class="model-repo">${escapeHtml(m.repo)}</span>
          </div>
          <div class="model-action" data-action="${escapeHtml(m.repo)}">
            ${m.downloaded
              ? '<span class="model-downloaded">✓ 已就绪</span>'
              : `<button class="btn-download" data-repo="${escapeHtml(m.repo)}" onclick="window.__modelsDownload(this.dataset.repo)">下载</button>`}
          </div>
        </div>
      `).join('');
    } catch (e) {
      listEl.innerHTML = `<div style="color: #ef4444; padding: 12px 0;">加载失败: ${escapeHtml(e)}</div>`;
    }
  }

  // 渲染某 repo 的下载中进度（替换该行的 action 区）。
  function renderProgress(repo, fileText, downloaded, total, speed, failed) {
    const actionEl = document.querySelector(`.model-action[data-action="${CSS.escape(repo)}"]`);
    if (!actionEl) return;
    if (failed) {
      actionEl.innerHTML = `<div class="progress-wrap"><div class="progress-failed">失败: ${escapeHtml(failed)}</div></div>`;
      return;
    }
    let pct = 0;
    if (total && total > 0) pct = Math.min(100, (downloaded / total) * 100);
    const speedText = speed ? ` · ${(speed / 1048576).toFixed(2)} MB/s` : '';
    const totalText = total ? `${fmtBytes(downloaded)}/${fmtBytes(total)}` : fmtBytes(downloaded);
    actionEl.innerHTML = `
      <div class="progress-wrap">
        ${fileText ? `<div class="progress-file">${escapeHtml(fileText)}</div>` : ''}
        <div class="progress-bar"><div class="progress-fill" style="width: ${pct.toFixed(1)}%"></div></div>
        <div class="progress-text">${totalText}${speedText} · ${pct.toFixed(0)}%</div>
      </div>`;
  }

  async function startDownload(repo) {
    if (currentRepo) { toast('已有下载进行中，请等待完成'); return; }
    currentRepo = repo;
    document.querySelectorAll('.btn-download').forEach((b) => (b.disabled = true));
    renderProgress(repo, '准备中...', 0, null, null);

    if (progressUnlisten) { progressUnlisten(); progressUnlisten = null; }
    if (fileUnlisten) { fileUnlisten(); fileUnlisten = null; }
    progressUnlisten = await listen('download-progress', (ev) => {
      const p = ev.payload;
      if (p.repo !== repo) return;
      renderProgress(repo, null, p.downloaded, p.total, p.speed);
    });
    fileUnlisten = await listen('download-file', (ev) => {
      const p = ev.payload;
      if (p.repo !== repo) return;
      renderProgress(repo, `文件 ${p.index}/${p.total} · ${p.file}`, 0, null, null);
    });

    try {
      await invoke('download_model', { repo });
      toast('下载完成: ' + repo);
    } catch (e) {
      toast('下载失败: ' + e);
      renderProgress(repo, null, 0, null, null, String(e));
    } finally {
      if (progressUnlisten) { progressUnlisten(); progressUnlisten = null; }
      if (fileUnlisten) { fileUnlisten(); fileUnlisten = null; }
      currentRepo = null;
      renderModels(); // 刷新 downloaded 状态（失败也刷新，恢复下载按钮）
    }
  }
  window.__modelsDownload = startDownload;

  async function initMirror() {
    const input = document.getElementById('mirror-input');
    if (!input || input.dataset.bound) return;
    input.dataset.bound = '1';
    try {
      const res = await invoke('get_config');
      input.value = (res.config && res.config.download_mirror) || '';
    } catch (e) { /* 读失败不阻塞，留空 */ }
    input.addEventListener('change', async () => {
      try {
        await invoke('set_download_mirror', { value: input.value.trim() });
        toast('镜像已保存');
      } catch (e) {
        toast('保存失败: ' + e);
      }
    });
  }

  window.initModelsPage = function () {
    injectStyles();
    initMirror();
    renderModels();
  };

  // 点击「模型管理」导航时初始化页面——挂自己的 click 监听，不改 index.html 的 switchPage。
  // （switchPage 已通过 onclick 触发；addEventListener 再叠一个，二者都执行，不冲突。）
  const navModels = document.querySelector('.nav-item[data-page="models"]');
  if (navModels) navModels.addEventListener('click', () => window.initModelsPage());
})();
