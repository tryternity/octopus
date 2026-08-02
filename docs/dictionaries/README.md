# 公开热词词典

来源：[THUOCL 清华大学开放中文词库](https://github.com/thunlp/THUOCL)
格式：`词\tDF值`（DF = Document Frequency，文档频次，来自大规模语料统计）

## 文件清单

| 文件 | 领域 | 词数 | 用途 |
|---|---|---|---|
| `THUOCL_IT.txt` | 计算机/IT | 16,000 | 计算机开发领域（字符串、数组、初始化、配置文件等） |
| `THUOCL_chengyu.txt` | 成语 | 8,519 | 日常交流/润色（坚定不移、随时随地等） |
| `THUOCL_food.txt` | 饮食 | 8,974 | 日常交流（土豆、苹果、蛋糕等） |
| `THUOCL_medical.txt` | 医学 | 18,749 | 医疗健康领域 |
| `THUOCL_law.txt` | 法律 | 9,896 | 法律专业领域 |

## 导入 octopus 热词

octopus 热词词数上限：单词典 20,000 词（`HOTWORD_SET_MAX_WORDS`）。

THUOCL_IT.txt（16,000 词）可直接导入一个词典。
其他文件按需导入或合并（注意不超 20,000 上限）。

DF 值可作为排序参考（值越高 = 该词在语料中越常见）。
导入时 octopus 会自动计算拼音（无需手动提供）。

## 许可

THUOCL 采用 [MIT 许可证](https://github.com/thunlp/THUOCL/blob/master/LICENSE)，可自由使用。
