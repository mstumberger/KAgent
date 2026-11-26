// A local draft of a settings value (ui-settings.md section 5). Text forms keep what
// the user types across store updates: a draft with unsaved edits stays, a clean one
// follows the store.
import { useState, type Dispatch, type SetStateAction } from "react";

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** `[draft, setDraft, dirty]` for the settings value `value`. */
export function useDraft<T>(value: T): [T, Dispatch<SetStateAction<T>>, boolean] {
  // `base` is the stored value the draft was last in step with.
  const [st, setSt] = useState(() => ({ base: value, draft: value }));
  if (!same(st.base, value)) {
    // The store moved: a save answered, or another window changed it. A clean draft
    // follows it, one with unsaved edits keeps them. React renders again at once with
    // the new state (react.dev, "Storing information from previous renders").
    const keep = !same(st.draft, st.base) && !same(st.draft, value);
    setSt({ base: value, draft: keep ? st.draft : value });
  }
  const setDraft: Dispatch<SetStateAction<T>> = (next) =>
    setSt({ base: value, draft: typeof next === "function" ? (next as (prev: T) => T)(st.draft) : next });
  return [st.draft, setDraft, !same(st.draft, value)];
}
