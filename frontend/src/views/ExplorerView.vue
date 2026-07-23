<script setup>
// File-system explorer: breadcrumb navigation, single click (or tap) opens,
// multi-select via rubber-band (mouse), Ctrl+click, or touch long-press
// (selection mode: taps toggle), keyboard (Delete, Ctrl+C/X/V), drag & drop,
// per-item three-dot menu (share / rename / delete), a markdown editor
// dialog, and live updates via SSE for the current project.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  eventStream,
  fsCopy,
  fsDelete,
  fsList,
  fsMkdir,
  fsMove,
  fsRead,
  fsRename,
  fsWrite,
  getDirectory,
  sharesCreate,
  sharesDelete,
  sharesList,
} from "../api.js";
import { renderMarkdown } from "../markdown.js";
import { modalBackdropClose, modalPress } from "../modal.js";

const route = useRoute();
const router = useRouter();

// ---- path & listing -------------------------------------------------------

const segments = computed(() => {
  const p = route.params.path;
  const arr = Array.isArray(p) ? p : p ? [p] : [];
  return arr.filter(Boolean);
});
const currentPath = computed(() => segments.value.join("/"));
const project = computed(() => segments.value[0] || "");

const entries = ref([]);
const loading = ref(false);
const error = ref("");
let prevDepth = 0;

const toast = ref(null);
let toastTimer = null;
function showToast(message, kind) {
  toast.value = { message, kind };
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.value = null), 3200);
}

async function load({ autoDescend = false } = {}) {
  loading.value = true;
  error.value = "";
  try {
    const data = await fsList(currentPath.value);
    entries.value = data.entries;
    // A project with a single user folder opens directly into it.
    if (
      autoDescend &&
      segments.value.length === 1 &&
      data.entries.length === 1 &&
      data.entries[0].is_dir
    ) {
      router.replace(`/x/${data.entries[0].path}`);
    }
  } catch (e) {
    error.value = (e && e.message) || "Failed to load this folder.";
    entries.value = [];
  } finally {
    loading.value = false;
  }
}

function goTo(path) {
  clearSelection();
  router.push(`/x/${path}`);
}

// Re-entry guard: the second click of a habitual double-click would land on
// whatever card occupies the same grid cell after the navigation re-render.
let lastOpenAt = 0;

function openEntry(entry) {
  const now = Date.now();
  if (now - lastOpenAt < 400) return;
  lastOpenAt = now;
  if (entry.is_dir) {
    goTo(entry.path);
  } else {
    openEditor(entry);
  }
}

watch(currentPath, () => {
  const depth = segments.value.length;
  const descending = depth > prevDepth;
  prevDepth = depth;
  clearSelection();
  clearFilter();
  closeMenu();
  load({ autoDescend: descending });
});

// ---- view options: sort + filter ------------------------------------------

const sortKey = ref(localStorage.getItem("memories_sort_key") || "name"); // "name" | "edited"
const sortDesc = ref(localStorage.getItem("memories_sort_dir") === "desc");
const filterInput = ref("");
const filterQuery = ref("");
const filterEl = ref(null);
let filterTimer = null;

function toggleSortKey() {
  sortKey.value = sortKey.value === "name" ? "edited" : "name";
  localStorage.setItem("memories_sort_key", sortKey.value);
}

function toggleSortDir() {
  sortDesc.value = !sortDesc.value;
  localStorage.setItem("memories_sort_dir", sortDesc.value ? "desc" : "asc");
}

// Debounce: apply the filter only after 0.5s without changes. v-model reacts
// to the `input` event, so typing, right-click paste and drag-drop into the
// field all funnel through this watcher.
watch(filterInput, (value) => {
  if (filterTimer) window.clearTimeout(filterTimer);
  filterTimer = window.setTimeout(() => (filterQuery.value = value), 500);
});

function clearFilter(refocus = false) {
  if (filterTimer) window.clearTimeout(filterTimer);
  filterTimer = null;
  filterInput.value = "";
  filterQuery.value = "";
  if (refocus) filterEl.value?.focus();
}

// What the grid shows: current folder entries, filtered by name substring,
// folders always first, each group sorted by the chosen key + direction.
const displayed = computed(() => {
  const q = filterQuery.value.trim().toLowerCase();
  const list = q
    ? entries.value.filter((e) => e.name.toLowerCase().includes(q))
    : entries.value.slice();
  const dir = sortDesc.value ? -1 : 1;
  const byName = (a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
  list.sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    const cmp =
      sortKey.value === "edited"
        ? (Date.parse(a.modified_at) || 0) - (Date.parse(b.modified_at) || 0)
        : byName(a, b);
    // Tiebreak stays ascending by name even in descending order.
    return dir * cmp || byName(a, b);
  });
  return list;
});

// Applying a filter prunes the selection to what stays visible, so bulk
// actions never touch hidden items.
watch(filterQuery, () => {
  if (!selected.value.size) return;
  const visible = new Set(displayed.value.map((e) => e.path));
  selected.value = new Set([...selected.value].filter((p) => visible.has(p)));
});

// ---- live updates (SSE) ---------------------------------------------------

const live = ref(false);
let source = null;
let reloadTimer = null;

function subscribe() {
  unsubscribe();
  if (!project.value) return;
  source = eventStream(project.value);
  source.addEventListener("change", () => {
    // Debounce bursts (a move emits several events).
    if (reloadTimer) window.clearTimeout(reloadTimer);
    reloadTimer = window.setTimeout(() => load(), 350);
  });
  source.onopen = () => (live.value = true);
  source.onerror = () => (live.value = false);
}

function unsubscribe() {
  if (source) {
    source.close();
    source = null;
  }
  live.value = false;
}

watch(project, subscribe);

// ---- selection ------------------------------------------------------------

const selected = ref(new Set());
const explorerEl = ref(null);
const rect = ref(null); // {x, y, w, h} in explorer-local coordinates
let dragOrigin = null;

function isSelected(path) {
  return selected.value.has(path);
}
function clearSelection() {
  selected.value = new Set();
}
function setSelection(paths) {
  selected.value = new Set(paths);
}

function togglePath(path) {
  const next = new Set(selected.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  selected.value = next;
}

// A single click (or tap) opens; selecting is Ctrl+click, the rubber band,
// or — on touch — a long-press that enters selection mode (taps then toggle).
function cardClick(entry, event) {
  if (suppressClick) {
    suppressClick = false;
    return;
  }
  if (event.ctrlKey || event.metaKey || selectMode.value) {
    togglePath(entry.path);
  } else {
    openEntry(entry);
  }
}

// ---- touch long-press -> selection mode -----------------------------------

const selectMode = ref(false);
let pressTimer = null;
let pressStartPos = null;
let pressPointerType = "mouse";
let suppressClick = false;

// Dropping the last selected item also leaves selection mode.
watch(selected, (s) => {
  if (!s.size) selectMode.value = false;
});

function pressStart(entry, event) {
  pressCancel();
  suppressClick = false;
  pressPointerType = event.pointerType || "mouse";
  // Native drag & drop stays a mouse feature: on touch a held draggable
  // would start an OS drag and swallow the long-press.
  event.currentTarget.draggable = pressPointerType === "mouse";
  if (pressPointerType === "mouse") return;
  if (event.target.closest(".menu-btn")) return;
  pressStartPos = { x: event.clientX, y: event.clientY };
  pressTimer = window.setTimeout(() => {
    pressTimer = null;
    suppressClick = true; // swallow the click that follows the release
    selectMode.value = true;
    const next = new Set(selected.value);
    next.add(entry.path);
    selected.value = next;
    navigator.vibrate?.(15);
  }, 450);
}

function pressMove(event) {
  if (!pressTimer || !pressStartPos) return;
  const moved =
    Math.abs(event.clientX - pressStartPos.x) > 10 ||
    Math.abs(event.clientY - pressStartPos.y) > 10;
  if (moved) pressCancel();
}

function pressCancel() {
  if (pressTimer) {
    window.clearTimeout(pressTimer);
    pressTimer = null;
  }
  pressStartPos = null;
}

// Rubber-band: mousedown on the explorer background starts a selection rect.
function bandStart(event) {
  if (event.button !== 0) return;
  if (event.target.closest(".file-card, .icon-btn, .btn")) return;
  const bounds = explorerEl.value.getBoundingClientRect();
  dragOrigin = { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  rect.value = { x: dragOrigin.x, y: dragOrigin.y, w: 0, h: 0 };
  if (!(event.ctrlKey || event.metaKey)) clearSelection();
  window.addEventListener("mousemove", bandMove);
  window.addEventListener("mouseup", bandEnd, { once: true });
}

function bandMove(event) {
  if (!dragOrigin) return;
  const bounds = explorerEl.value.getBoundingClientRect();
  const cur = { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  rect.value = {
    x: Math.min(dragOrigin.x, cur.x),
    y: Math.min(dragOrigin.y, cur.y),
    w: Math.abs(cur.x - dragOrigin.x),
    h: Math.abs(cur.y - dragOrigin.y),
  };
  // Live-intersect the rectangle with every card.
  const hits = [];
  for (const card of explorerEl.value.querySelectorAll(".file-card[data-path]")) {
    const cb = card.getBoundingClientRect();
    const local = {
      x: cb.left - bounds.left,
      y: cb.top - bounds.top,
      w: cb.width,
      h: cb.height,
    };
    const r = rect.value;
    const overlap =
      local.x < r.x + r.w && local.x + local.w > r.x && local.y < r.y + r.h && local.y + local.h > r.y;
    if (overlap) hits.push(card.dataset.path);
  }
  setSelection(hits);
}

function bandEnd() {
  window.removeEventListener("mousemove", bandMove);
  dragOrigin = null;
  rect.value = null;
}

// ---- clipboard (Ctrl+C / Ctrl+X / Ctrl+V) ---------------------------------

const clipboard = ref(null); // { paths: [], mode: "copy" | "cut" }

const cutSet = computed(() =>
  clipboard.value && clipboard.value.mode === "cut" ? new Set(clipboard.value.paths) : new Set(),
);

function doCopy() {
  if (!selected.value.size) return;
  clipboard.value = { paths: [...selected.value], mode: "copy" };
  showToast(`${selected.value.size} item(s) ready to copy.`, "success");
}
function doCut() {
  if (!selected.value.size) return;
  clipboard.value = { paths: [...selected.value], mode: "cut" };
  showToast(`${selected.value.size} item(s) ready to move.`, "success");
}
async function doPaste() {
  if (!clipboard.value || !clipboard.value.paths.length) return;
  const { paths, mode } = clipboard.value;
  try {
    if (mode === "cut") {
      await fsMove(paths, currentPath.value);
      clipboard.value = null;
    } else {
      await fsCopy(paths, currentPath.value);
    }
    await load();
    showToast(mode === "cut" ? "Moved." : "Copied.", "success");
  } catch (e) {
    showToast((e && e.message) || "Paste failed.", "error");
  }
}

async function deleteSelection(paths) {
  const list = paths || [...selected.value];
  if (!list.length) return;
  const label = list.length === 1 ? `"${list[0].split("/").pop()}"` : `${list.length} items`;
  if (!window.confirm(`Delete ${label}? This cannot be undone.`)) return;
  try {
    await fsDelete(list);
    clearSelection();
    await load();
    showToast("Deleted.", "success");
  } catch (e) {
    showToast((e && e.message) || "Delete failed.", "error");
  }
}

function onKeydown(event) {
  // Don't hijack keys while typing in inputs/dialogs.
  const tag = (event.target.tagName || "").toLowerCase();
  if (tag === "input" || tag === "textarea" || event.target.isContentEditable) return;
  if (document.querySelector("dialog[open]")) return;
  if (event.key === "Delete") {
    event.preventDefault();
    deleteSelection();
  } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "c") {
    event.preventDefault();
    doCopy();
  } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "x") {
    event.preventDefault();
    doCut();
  } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v") {
    event.preventDefault();
    doPaste();
  } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
    event.preventDefault();
    setSelection(displayed.value.map((e) => e.path));
  } else if (event.key === "Escape") {
    clearSelection();
    closeMenu();
  }
}

// ---- drag & drop ----------------------------------------------------------

const dropTarget = ref("");

function dragStart(entry, event) {
  const paths = isSelected(entry.path) ? [...selected.value] : [entry.path];
  event.dataTransfer.setData("application/x-memories-paths", JSON.stringify(paths));
  event.dataTransfer.effectAllowed = "move";
}

function dragOver(target, event) {
  // target: an entry (folder) or a breadcrumb path string.
  const path = typeof target === "string" ? target : target.is_dir ? target.path : null;
  if (path === null) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "move";
  dropTarget.value = path;
}

function dragLeave() {
  dropTarget.value = "";
}

async function drop(target, event) {
  const path = typeof target === "string" ? target : target.path;
  dropTarget.value = "";
  const raw = event.dataTransfer.getData("application/x-memories-paths");
  if (!raw) return;
  event.preventDefault();
  let paths;
  try {
    paths = JSON.parse(raw);
  } catch {
    return;
  }
  paths = paths.filter((p) => p !== path && !path.startsWith(`${p}/`));
  if (!paths.length) return;
  try {
    await fsMove(paths, path);
    clearSelection();
    await load();
    showToast("Moved.", "success");
  } catch (e) {
    showToast((e && e.message) || "Move failed.", "error");
  }
}

// ---- three-dot menu -------------------------------------------------------

const menu = ref(null); // { entry, x, y }

function openMenu(entry, event) {
  event.stopPropagation();
  const r = event.currentTarget.getBoundingClientRect();
  menu.value = {
    entry,
    x: Math.min(r.left, window.innerWidth - 200),
    y: Math.min(r.bottom + 4, window.innerHeight - 160),
  };
}
function closeMenu() {
  menu.value = null;
}

// ---- dialogs --------------------------------------------------------------

const nameDialog = ref(null); // { title, value, submit }
const nameInput = ref("");
const nameDialogEl = ref(null);

function promptName(title, initial, submit) {
  nameDialog.value = { title, submit };
  nameInput.value = initial;
  nextTick(() => nameDialogEl.value?.showModal());
}

async function nameSubmit() {
  const value = nameInput.value.trim();
  if (!value) return;
  try {
    await nameDialog.value.submit(value);
    nameDialogEl.value?.close();
    nameDialog.value = null;
    await load();
  } catch (e) {
    showToast((e && e.message) || "Operation failed.", "error");
  }
}

function newFolder() {
  promptName(segments.value.length ? "New folder" : "New project", "", (v) =>
    fsMkdir(currentPath.value ? `${currentPath.value}/${v}` : v),
  );
}

function newFile() {
  promptName("New memory file", "notes.md", (v) => fsWrite(`${currentPath.value}/${v}`, ""));
}

function renameItem(entry) {
  closeMenu();
  promptName("Rename", entry.name, (v) => fsRename(entry.path, v));
}

// Markdown editor. `mode` is "preview" (rendered, read-only) or "raw" (the
// editable textarea); preview is the default because it is far more readable.
const editor = ref(null); // { path, name, content, saving, mode }
const editorEl = ref(null);

const editorHtml = computed(() =>
  editor.value ? renderMarkdown(editor.value.content) : "",
);

function toggleEditorMode() {
  if (!editor.value) return;
  editor.value.mode = editor.value.mode === "preview" ? "raw" : "preview";
}

async function openEditor(entry) {
  try {
    const data = await fsRead(entry.path);
    editor.value = {
      path: entry.path,
      name: entry.name,
      content: data.content,
      saving: false,
      mode: "preview",
    };
    nextTick(() => editorEl.value?.showModal());
  } catch (e) {
    showToast((e && e.message) || "Failed to open the file.", "error");
  }
}

async function saveEditor() {
  if (!editor.value) return;
  editor.value.saving = true;
  try {
    await fsWrite(editor.value.path, editor.value.content);
    showToast("Saved.", "success");
    editorEl.value?.close();
    editor.value = null;
  } catch (e) {
    showToast((e && e.message) || "Save failed.", "error");
  } finally {
    if (editor.value) editor.value.saving = false;
  }
}

// Share dialog.
const share = ref(null); // { entry, shares, users, groups, granteeKey, permission }
const shareEl = ref(null);

async function openShare(entry) {
  closeMenu();
  try {
    const [dir, existing] = await Promise.all([getDirectory(), sharesList(entry.path)]);
    share.value = {
      entry,
      shares: existing.shares,
      users: dir.users,
      groups: dir.groups,
      granteeKey: "",
      permission: "read",
    };
    nextTick(() => shareEl.value?.showModal());
  } catch (e) {
    showToast((e && e.message) || "Failed to load sharing.", "error");
  }
}

async function addShare() {
  const s = share.value;
  if (!s || !s.granteeKey) return;
  const [type, id] = s.granteeKey.split(":");
  try {
    const res = await sharesCreate({
      path: s.entry.path,
      grantee_type: type,
      grantee_id: id,
      permission: s.permission,
    });
    s.shares = res.shares;
    await load();
  } catch (e) {
    showToast((e && e.message) || "Share failed.", "error");
  }
}

async function removeShare(item) {
  try {
    await sharesDelete(item.id);
    share.value.shares = share.value.shares.filter((x) => x.id !== item.id);
    await load();
  } catch (e) {
    showToast((e && e.message) || "Failed to remove the share.", "error");
  }
}

// ---- lifecycle ------------------------------------------------------------

onMounted(() => {
  prevDepth = segments.value.length;
  load({ autoDescend: true });
  subscribe();
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("click", closeMenu);
});

onBeforeUnmount(() => {
  unsubscribe();
  pressCancel();
  if (filterTimer) window.clearTimeout(filterTimer);
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("click", closeMenu);
  window.removeEventListener("mousemove", bandMove);
});
</script>

<template>
  <section>
    <!-- Breadcrumb + actions -->
    <div class="panel-head" style="margin-top: 6px">
      <nav class="crumbs" aria-label="Breadcrumb">
        <span
          class="crumb"
          :class="{ current: !segments.length, 'drop-target': dropTarget === '' && segments.length > 0 }"
          @click="segments.length && goTo('')"
          @dragover="segments.length && dragOver('', $event)"
          @dragleave="dragLeave"
          @drop="drop('', $event)"
        >🏠 Projects</span>
        <template v-for="(seg, i) in segments" :key="i">
          <span class="crumb-sep">›</span>
          <span
            class="crumb"
            :class="{
              current: i === segments.length - 1,
              'drop-target': dropTarget === segments.slice(0, i + 1).join('/') && i < segments.length - 1,
            }"
            @click="i < segments.length - 1 && goTo(segments.slice(0, i + 1).join('/'))"
            @dragover="i < segments.length - 1 && dragOver(segments.slice(0, i + 1).join('/'), $event)"
            @dragleave="dragLeave"
            @drop="drop(segments.slice(0, i + 1).join('/'), $event)"
          >{{ seg }}</span>
        </template>
        <span v-if="live" class="chip rw" style="margin-left: 10px" title="Live updates connected">● live</span>
      </nav>
      <div class="panel-head-actions">
        <div class="btn-group">
          <div class="filter-wrap">
            <input
              ref="filterEl"
              v-model="filterInput"
              type="text"
              class="filter-input"
              placeholder="Filter…"
              aria-label="Filter items in this folder"
              @keydown.esc.stop="clearFilter(true)"
            />
            <button
              v-if="filterInput"
              type="button"
              class="icon-btn small filter-clear"
              aria-label="Clear filter"
              @click="clearFilter(true)"
            >✕</button>
          </div>
          <button
            type="button"
            class="btn ghost small"
            :title="sortKey === 'name' ? 'Sorted by name — switch to last edit' : 'Sorted by last edit — switch to name'"
            @click="toggleSortKey"
          >{{ sortKey === "name" ? "🔤 Name" : "🕒 Edited" }}</button>
          <button
            type="button"
            class="icon-btn small"
            :title="sortDesc ? 'Descending — switch to ascending' : 'Ascending — switch to descending'"
            :aria-label="sortDesc ? 'Sort descending' : 'Sort ascending'"
            @click="toggleSortDir"
          >{{ sortDesc ? "↓" : "↑" }}</button>
        </div>
        <div class="btn-group">
          <button v-if="clipboard" type="button" class="btn ghost small" @click="doPaste">
            📋 Paste ({{ clipboard.paths.length }})
          </button>
          <button v-if="segments.length" type="button" class="btn ghost small" @click="newFile">+ File</button>
          <button type="button" class="btn primary small" @click="newFolder">
            {{ segments.length ? "+ Folder" : "+ Project" }}
          </button>
        </div>
      </div>
    </div>

    <p v-if="error" class="alert" role="alert">{{ error }}</p>

    <!-- Explorer canvas -->
    <div ref="explorerEl" class="explorer" @mousedown="bandStart">
      <div v-if="loading && !entries.length" class="empty-dir">Loading…</div>
      <div v-else-if="!entries.length" class="empty-dir">
        <p style="font-size: 34px; margin: 0 0 8px">🗂️</p>
        <p>{{ segments.length ? "This folder is empty." : "No projects yet — create one!" }}</p>
      </div>
      <div v-else-if="!displayed.length" class="empty-dir">
        <p style="font-size: 34px; margin: 0 0 8px">🔍</p>
        <p>Nothing here matches “{{ filterQuery }}”.</p>
      </div>
      <div v-else class="file-grid">
        <div
          v-for="entry in displayed"
          :key="entry.path"
          class="file-card"
          :class="{
            selected: isSelected(entry.path),
            cut: cutSet.has(entry.path),
            'drop-target': dropTarget === entry.path,
            'menu-open': menu && menu.entry.path === entry.path,
          }"
          :data-path="entry.path"
          draggable="true"
          @mousedown.stop
          @click.stop="cardClick(entry, $event)"
          @pointerdown="pressStart(entry, $event)"
          @pointermove="pressMove"
          @pointerup="pressCancel"
          @pointercancel="pressCancel"
          @contextmenu="pressPointerType !== 'mouse' && $event.preventDefault()"
          @dragstart="dragStart(entry, $event)"
          @dragover="dragOver(entry, $event)"
          @dragleave="dragLeave"
          @drop="drop(entry, $event)"
        >
          <span class="f-badges">
            <span v-if="entry.shared" class="chip shared" title="Shared">🔗</span>
            <span v-if="entry.access === 'read'" class="chip ro" title="Read-only">👁</span>
          </span>
          <button
            type="button"
            class="icon-btn small menu-btn"
            aria-label="Item menu"
            @click.stop="openMenu(entry, $event)"
            @mousedown.stop
          >⋮</button>
          <span class="f-icon">{{ entry.is_dir ? (segments.length === 0 ? "🗂️" : "📁") : "📄" }}</span>
          <span class="f-name">{{ entry.name }}</span>
          <span v-if="!entry.is_dir && entry.size" class="f-sub">{{ (entry.size / 1024).toFixed(1) }} kB</span>
        </div>
      </div>

      <!-- Rubber-band rectangle -->
      <div
        v-if="rect"
        class="select-rect"
        :style="{ left: rect.x + 'px', top: rect.y + 'px', width: rect.w + 'px', height: rect.h + 'px' }"
      ></div>
    </div>

    <!-- Selection action bar -->
    <div v-if="selected.size" class="sel-bar">
      <span class="count">{{ selected.size }} selected</span>
      <button type="button" class="btn ghost small" @click="doCopy">Copy</button>
      <button type="button" class="btn ghost small" @click="doCut">Cut</button>
      <button type="button" class="btn danger-btn small" @click="deleteSelection()">Delete</button>
      <button type="button" class="icon-btn small" aria-label="Clear selection" @click="clearSelection">✕</button>
    </div>

    <!-- Three-dot menu -->
    <div v-if="menu" class="ctx-menu" :style="{ left: menu.x + 'px', top: menu.y + 'px' }" @click.stop>
      <button type="button" @click="openShare(menu.entry)">🔗 Share…</button>
      <button type="button" @click="renameItem(menu.entry)">✏️ Rename…</button>
      <button type="button" class="danger" @click="closeMenu(); deleteSelection([menu.entry.path])">🗑 Delete</button>
    </div>

    <!-- Name dialog (new folder / file / rename) -->
    <dialog ref="nameDialogEl" class="mdl" @mousedown="modalPress" @click="modalBackdropClose" @close="nameDialog = null">
      <form v-if="nameDialog" @submit.prevent="nameSubmit">
        <div class="mdl-head"><h2>{{ nameDialog.title }}</h2></div>
        <div class="mdl-body">
          <input v-model="nameInput" type="text" aria-label="Name" required autofocus />
        </div>
        <div class="mdl-actions">
          <button type="button" class="btn ghost" @click="nameDialogEl.close()">Cancel</button>
          <button type="submit" class="btn primary">OK</button>
        </div>
      </form>
    </dialog>

    <!-- Markdown editor -->
    <dialog ref="editorEl" class="mdl wide" @mousedown="modalPress" @click="modalBackdropClose" @close="editor = null">
      <div v-if="editor">
        <div class="mdl-head">
          <div class="mdl-head-title">
            <h2>{{ editor.name }}</h2>
            <span class="muted" style="font-size: 12.5px">{{ editor.path }}</span>
          </div>
          <button
            type="button"
            class="btn ghost small"
            :title="editor.mode === 'preview' ? 'Editing the raw markdown' : 'Rendered markdown preview'"
            @click="toggleEditorMode"
          >{{ editor.mode === "preview" ? "✏️ Edit raw" : "👁 Preview" }}</button>
        </div>
        <div class="mdl-body">
          <textarea
            v-if="editor.mode === 'raw'"
            v-model="editor.content"
            class="code-editor"
            rows="18"
            aria-label="File content"
          ></textarea>
          <!-- Rendered from sanitised, HTML-escaped markdown (see markdown.js). -->
          <div v-else class="md-preview" v-html="editorHtml"></div>
        </div>
        <div class="mdl-actions">
          <button type="button" class="btn ghost" @click="editorEl.close()">Close</button>
          <button type="button" class="btn primary" :disabled="editor.saving" @click="saveEditor">
            {{ editor.saving ? "Saving…" : "Save" }}
          </button>
        </div>
      </div>
    </dialog>

    <!-- Share dialog -->
    <dialog ref="shareEl" class="mdl" @mousedown="modalPress" @click="modalBackdropClose" @close="share = null">
      <div v-if="share">
        <div class="mdl-head"><h2>Share “{{ share.entry.name }}”</h2></div>
        <div class="mdl-body">
          <div class="field">
            <label for="share-grantee">Share with</label>
            <select id="share-grantee" v-model="share.granteeKey">
              <option value="" disabled>Choose a user or group…</option>
              <optgroup label="Users">
                <option v-for="u in share.users" :key="u.id" :value="`user:${u.id}`">{{ u.username }}</option>
              </optgroup>
              <optgroup v-if="share.groups.length" label="Groups">
                <option v-for="g in share.groups" :key="g.id" :value="`group:${g.id}`">{{ g.name }}</option>
              </optgroup>
            </select>
          </div>
          <div class="field">
            <label for="share-perm">Permission</label>
            <select id="share-perm" v-model="share.permission">
              <option value="read">Read only</option>
              <option value="readwrite">Read &amp; write</option>
            </select>
          </div>
          <button type="button" class="btn primary" :disabled="!share.granteeKey" @click="addShare">Add</button>

          <h3 style="margin-top: 18px">Current shares</h3>
          <p v-if="!share.shares.length" class="muted">Not shared with anyone yet.</p>
          <div v-for="s in share.shares" :key="s.id" class="share-row">
            <span>
              {{ s.grantee_type === "group" ? "👥" : "👤" }} <strong>{{ s.grantee_name }}</strong>
              <span class="chip" :class="s.permission === 'readwrite' ? 'rw' : 'ro'" style="margin-left: 8px">
                {{ s.permission === "readwrite" ? "read & write" : "read only" }}
              </span>
            </span>
            <button type="button" class="icon-btn small danger" aria-label="Remove share" @click="removeShare(s)">✕</button>
          </div>
        </div>
        <div class="mdl-actions">
          <button type="button" class="btn ghost" @click="shareEl.close()">Done</button>
        </div>
      </div>
    </dialog>

    <div v-if="toast" class="toast" :class="toast.kind" role="status">{{ toast.message }}</div>
  </section>
</template>
