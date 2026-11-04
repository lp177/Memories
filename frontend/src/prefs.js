// Per-browser UI preferences, kept in localStorage and shared across views
// through module-level refs. Storage can be unavailable (private mode, blocked
// site data), so every access falls back to the default.
import { ref, watch } from "vue";

function stored(key, fallback) {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : JSON.parse(raw);
  } catch {
    return fallback;
  }
}

function persistedRef(key, fallback) {
  const r = ref(stored(key, fallback));
  watch(r, (v) => {
    try {
      localStorage.setItem(key, JSON.stringify(v));
    } catch {
      /* not persisted: the setting still applies to this page load */
    }
  });
  return r;
}

// Ask "Delete …? This cannot be undone." before deleting files and folders.
export const confirmDelete = persistedRef("memories_confirm_delete", true);
