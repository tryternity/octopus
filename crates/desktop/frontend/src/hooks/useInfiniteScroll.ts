import { useEffect, useRef, type RefObject } from "react";

/**
 * 无限滚动：当 sentinel 元素进入视口时触发 onLoadMore。
 * 通过 loading 和 done 防止重复加载。
 */
export function useInfiniteScroll(
  onLoadMore: () => void,
  loading: boolean,
  done: boolean,
): RefObject<HTMLDivElement | null> {
  const sentinelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && !loading && !done) {
          onLoadMore();
        }
      },
      { rootMargin: "100px" },
    );

    observer.observe(el);
    return () => observer.disconnect();
  }, [onLoadMore, loading, done]);

  return sentinelRef;
}
