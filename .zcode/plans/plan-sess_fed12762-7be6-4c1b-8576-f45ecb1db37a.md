# 新增"动作"Tab（Alt+Z）——只匹配菜单项（source="menu"）

## 改动
1. **searchTypes.ts**：TabId 加 "actions"；TABS 数组末尾加 `{ id: "actions", label: "动作", key: "z" }`
2. **searchLogic.ts**：filterByTab 的 sourceMap 加 `actions: "menu"`
3. **测试**：getTabByKey("z")/getNextTab 循环/filterByTab("actions") 只留 source="menu"
4. **spec**：§3.4 Tab 表 + §3.5.1 搜索模式表

前端 filterByTab 过滤掉 quicklink（只留 source="menu"），后端不改。Tab 循环：all→apps→files→shell→bookmarks→actions→all。