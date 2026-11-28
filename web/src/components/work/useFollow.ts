// Following the end of a scrolling body while the owner is at the bottom (work-windows.md sections
// 5.4): scrolled up by more than 8 px it stops, and a floating button resumes it.
import { useCallback, useEffect, useRef, useState } from "react";

export const BOTTOM_PX = 8;

/** `ref` goes on the scrolling element; `dep` changes when its content does. */
export function useFollow<T extends HTMLElement>(dep: unknown) {
  const ref = useRef<T>(null);
  const [following, setFollowing] = useState(true);
  const follow = useRef(true);
  const onScroll = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const atEnd = el.scrollHeight - el.scrollTop - el.clientHeight <= BOTTOM_PX;
    follow.current = atEnd;
    setFollowing(atEnd);
  }, []);
  useEffect(() => {
    const el = ref.current;
    if (el && follow.current) el.scrollTop = el.scrollHeight;
  }, [dep]);
  const resume = useCallback(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
    follow.current = true;
    setFollowing(true);
  }, []);
  return { ref, following, onScroll, resume };
}
