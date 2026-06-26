#!/usr/bin/env python3
"""
拼音纠错器数据生成器（手动运行，不属于构建流程）。

下载并处理两个语料，产出嵌入二进制的纠错数据：
  - jieba dict.txt.big       → top 40k 词 → src/corrector_data/unigram.txt.gz
  - gotokenizer bigram.txt   → top 40k 对 → src/corrector_data/bigram.txt.gz

产物（.gz）已提交仓库，由 crates/asr/src/corrector.rs 在编译期经
include_bytes! 嵌入 LightCorrector。平时无需重跑；仅当需要更新语料
（换更新的 jieba dict / 调 top-N 阈值 / 换镜像源）时手动执行：

    python3 crates/asr/scripts/generate_corrector_data.py

输出路径按脚本自身位置解析，可从任意目录运行。
"""
import urllib.request
import gzip
import os

# 产物目录相对本脚本位置（../src/corrector_data），与 corrector.rs 的 include_bytes! 路径一致。
HERE = os.path.dirname(os.path.abspath(__file__))
DATA_DIR = os.path.join(HERE, "..", "src", "corrector_data")


def download_file(url, desc):
    print(f"Downloading {desc} from {url}...")
    headers = {'User-Agent': 'Mozilla/5.0'}
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=30) as response:
        return response.read()

def main():
    os.makedirs(DATA_DIR, exist_ok=True)
    
    # 1. Download and process unigrams (jieba dict.txt.big)
    unigram_url = "https://fastly.jsdelivr.net/gh/fxsjy/jieba/extra_dict/dict.txt.big"
    try:
        unigram_data = download_file(unigram_url, "jieba dict.txt.big").decode('utf-8')
    except Exception as e:
        print(f"Failed to download from jsdelivr: {e}. Trying raw github...")
        unigram_url = "https://raw.githubusercontent.com/fxsjy/jieba/master/extra_dict/dict.txt.big"
        unigram_data = download_file(unigram_url, "jieba dict.txt.big").decode('utf-8')
        
    unigrams = []
    for line in unigram_data.strip().split('\n'):
        parts = line.strip().split()
        if len(parts) >= 2:
            word = parts[0]
            try:
                freq = int(parts[1])
                unigrams.append((word, freq))
            except ValueError:
                continue
                
    # Sort by frequency descending and keep top 40,000
    unigrams.sort(key=lambda x: x[1], reverse=True)
    top_unigrams = unigrams[:40000]
    
    # Write gzipped unigram file
    unigram_out_path = os.path.join(DATA_DIR, "unigram.txt.gz")
    with gzip.open(unigram_out_path, 'wt', encoding='utf-8') as f:
        for word, freq in top_unigrams:
            f.write(f"{word} {freq}\n")
    print(f"Saved {len(top_unigrams)} unigrams to {unigram_out_path} (size: {os.path.getsize(unigram_out_path)} bytes)")
    
    # 2. Download and process bigrams (gotokenizer bigram.txt)
    # Use mirror to bypass GFW/raw github issues in China
    bigram_url = "https://mirror.ghproxy.com/https://raw.githubusercontent.com/xujiajun/gotokenizer/master/data/zh/bigram.txt"
    try:
        bigram_data = download_file(bigram_url, "gotokenizer bigram.txt").decode('utf-8')
    except Exception as e:
        print(f"Failed to download from ghproxy: {e}. Trying raw github...")
        bigram_url = "https://raw.githubusercontent.com/xujiajun/gotokenizer/master/data/zh/bigram.txt"
        bigram_data = download_file(bigram_url, "gotokenizer bigram.txt").decode('utf-8')
        
    bigrams = []
    for line in bigram_data.strip().split('\n'):
        parts = line.strip().split()
        if len(parts) >= 2:
            pair = parts[0]
            try:
                freq = int(parts[1])
                if ':' in pair:
                    w1, w2 = pair.split(':', 1)
                    bigrams.append((w1, w2, freq))
            except ValueError:
                continue
                
    # Sort by frequency descending and keep top 40,000
    bigrams.sort(key=lambda x: x[2], reverse=True)
    top_bigrams = bigrams[:40000]
    
    # Write gzipped bigram file
    bigram_out_path = os.path.join(DATA_DIR, "bigram.txt.gz")
    with gzip.open(bigram_out_path, 'wt', encoding='utf-8') as f:
        for w1, w2, freq in top_bigrams:
            f.write(f"{w1}:{w2} {freq}\n")
    print(f"Saved {len(top_bigrams)} bigrams to {bigram_out_path} (size: {os.path.getsize(bigram_out_path)} bytes)")

if __name__ == "__main__":
    main()
