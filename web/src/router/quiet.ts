// A change of place that is not the owner's own (the next conversation after a delete, the first one opened at
// load): the router replaces the entry instead of pushing one (shell.md section 2). Its own module, so the store
// and the router can both import it.
let depth = 0;

/** Run `fn`; the place changes it makes are replaces. */
export function quiet<T>(fn: () => T): T {
  depth += 1;
  try {
    return fn();
  } finally {
    // The store notifies synchronously, so the router has seen the change by now.
    depth -= 1;
  }
}

export const isQuiet = () => depth > 0;
