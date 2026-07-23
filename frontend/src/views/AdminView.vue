<script setup>
// Admin panel: user management (create, edit pseudo/email/password, reset
// link, delete) and group management (create, membership, delete).
import { onMounted, ref } from "vue";
import {
  adminCreateGroup,
  adminCreateUser,
  adminDeleteGroup,
  adminDeleteUser,
  adminGroups,
  adminResetLink,
  adminUpdateGroup,
  adminUpdateUser,
  adminUsers,
} from "../api.js";
import { modalBackdropClose, modalPress } from "../modal.js";

const users = ref([]);
const groups = ref([]);
const error = ref("");

const toast = ref(null);
let toastTimer = null;
function showToast(message, kind) {
  toast.value = { message, kind };
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.value = null), 4200);
}

async function load() {
  try {
    users.value = (await adminUsers()).users;
    groups.value = (await adminGroups()).groups;
  } catch (e) {
    error.value = (e && e.message) || "Admin data failed to load (admin only).";
  }
}

// ---- users ----
const userDialogEl = ref(null);
const userDraft = ref(null); // {id?, username, email, password, is_admin}

function newUser() {
  userDraft.value = { username: "", email: "", password: "", is_admin: false };
  userDialogEl.value?.showModal();
}
function editUser(u) {
  userDraft.value = { id: u.id, username: u.username, email: u.email, password: "", is_admin: u.is_admin };
  userDialogEl.value?.showModal();
}

async function saveUser() {
  const d = userDraft.value;
  try {
    if (d.id) {
      const body = { username: d.username, email: d.email, is_admin: d.is_admin };
      if (d.password) body.password = d.password;
      await adminUpdateUser(d.id, body);
    } else {
      await adminCreateUser({ username: d.username, email: d.email, password: d.password, is_admin: d.is_admin });
    }
    userDialogEl.value?.close();
    userDraft.value = null;
    await load();
    showToast("User saved.", "success");
  } catch (e) {
    showToast((e && e.message) || "Failed to save the user.", "error");
  }
}

async function removeUser(u) {
  if (!window.confirm(`Delete user "${u.username}"? Their tokens and shares are removed too.`)) return;
  try {
    await adminDeleteUser(u.id);
    await load();
  } catch (e) {
    showToast((e && e.message) || "Failed to delete the user.", "error");
  }
}

const resetInfo = ref(null); // {username, link, mailed}
async function sendReset(u) {
  try {
    const res = await adminResetLink(u.id);
    resetInfo.value = { username: u.username, link: res.link, mailed: res.mailed };
  } catch (e) {
    showToast((e && e.message) || "Failed to create a reset link.", "error");
  }
}

async function copyResetLink() {
  try {
    await navigator.clipboard.writeText(resetInfo.value.link);
    showToast("Link copied.", "success");
  } catch {
    showToast("Copy failed — select the link manually.", "error");
  }
}

// ---- groups ----
const groupDialogEl = ref(null);
const groupDraft = ref(null); // {id?, name, members:Set}

function newGroup() {
  groupDraft.value = { name: "", members: new Set() };
  groupDialogEl.value?.showModal();
}
function editGroup(g) {
  groupDraft.value = { id: g.id, name: g.name, members: new Set(g.members) };
  groupDialogEl.value?.showModal();
}
function toggleMember(id) {
  const m = groupDraft.value.members;
  if (m.has(id)) m.delete(id);
  else m.add(id);
}

async function saveGroup() {
  const d = groupDraft.value;
  try {
    if (d.id) {
      await adminUpdateGroup(d.id, { name: d.name, members: [...d.members] });
    } else {
      await adminCreateGroup(d.name, [...d.members]);
    }
    groupDialogEl.value?.close();
    groupDraft.value = null;
    await load();
    showToast("Group saved.", "success");
  } catch (e) {
    showToast((e && e.message) || "Failed to save the group.", "error");
  }
}

async function removeGroup(g) {
  if (!window.confirm(`Delete group "${g.name}"? Shares granted to it are removed.`)) return;
  try {
    await adminDeleteGroup(g.id);
    await load();
  } catch (e) {
    showToast((e && e.message) || "Failed to delete the group.", "error");
  }
}

function usernameOf(id) {
  return users.value.find((u) => u.id === id)?.username || id;
}

onMounted(load);
</script>

<template>
  <section>
    <h1 style="margin-top: 8px">Administration</h1>
    <p v-if="error" class="alert" role="alert">{{ error }}</p>

    <!-- Users -->
    <section class="panel">
      <div class="panel-head">
        <h2>Users</h2>
        <button type="button" class="btn primary small" @click="newUser">+ Add user</button>
      </div>

      <div v-if="resetInfo" class="token-reveal">
        <strong>Reset link for {{ resetInfo.username }}</strong>
        <span v-if="resetInfo.mailed"> — also sent by e-mail.</span>
        <span v-else> — SMTP is not configured, hand this link to the user:</span>
        <div class="token-reveal-row">
          <input class="token-value" :value="resetInfo.link" readonly aria-label="Password reset link" @focus="$event.target.select()" />
          <button type="button" class="btn" @click="copyResetLink">Copy</button>
          <button type="button" class="btn ghost" @click="resetInfo = null">Done</button>
        </div>
      </div>

      <div class="list" role="table" aria-label="Users">
        <div class="list-head" role="row">
          <span role="columnheader" style="flex: 0 0 150px">Pseudo</span>
          <span role="columnheader" class="grow">E-mail</span>
          <span role="columnheader" style="flex: 0 0 70px">Role</span>
        </div>
        <div v-for="u in users" :key="u.id" class="list-row" role="row">
          <span role="cell" style="flex: 0 0 150px; font-weight: 600">{{ u.username }}</span>
          <span role="cell" class="grow muted">{{ u.email || "—" }}</span>
          <span role="cell" style="flex: 0 0 70px">
            <span v-if="u.is_admin" class="chip admin">admin</span><span v-else class="muted">user</span>
          </span>
          <span role="cell" class="btn-group">
            <button type="button" class="btn ghost small" @click="editUser(u)">Edit</button>
            <button type="button" class="btn ghost small" @click="sendReset(u)">Reset password</button>
            <button type="button" class="btn danger-btn small" @click="removeUser(u)">Delete</button>
          </span>
        </div>
      </div>
    </section>

    <!-- Groups -->
    <section class="panel">
      <div class="panel-head">
        <h2>Groups</h2>
        <button type="button" class="btn primary small" @click="newGroup">+ Add group</button>
      </div>
      <p class="field-hint" style="margin: 0 0 12px">
        Users have no group by default — only admins create groups and assign members. Groups can be
        used as share targets in the explorer.
      </p>
      <div v-if="groups.length" class="list" role="table" aria-label="Groups">
        <div class="list-head" role="row">
          <span role="columnheader" style="flex: 0 0 150px">Name</span>
          <span role="columnheader" class="grow">Members</span>
        </div>
        <div v-for="g in groups" :key="g.id" class="list-row" role="row">
          <span role="cell" style="flex: 0 0 150px; font-weight: 600">{{ g.name }}</span>
          <span role="cell" class="grow muted">
            {{ g.members.length ? g.members.map(usernameOf).join(", ") : "no members" }}
          </span>
          <span role="cell" class="btn-group">
            <button type="button" class="btn ghost small" @click="editGroup(g)">Edit</button>
            <button type="button" class="btn danger-btn small" @click="removeGroup(g)">Delete</button>
          </span>
        </div>
      </div>
      <p v-else class="muted">No groups yet.</p>
    </section>

    <!-- User dialog -->
    <dialog ref="userDialogEl" class="mdl" @mousedown="modalPress" @click="modalBackdropClose" @close="userDraft = null">
      <form v-if="userDraft" @submit.prevent="saveUser">
        <div class="mdl-head"><h2>{{ userDraft.id ? "Edit user" : "New user" }}</h2></div>
        <div class="mdl-body">
          <div class="field">
            <label for="u-name">Pseudo (username)</label>
            <input id="u-name" v-model="userDraft.username" type="text" required />
          </div>
          <div class="field">
            <label for="u-mail">E-mail</label>
            <input id="u-mail" v-model="userDraft.email" type="email" />
          </div>
          <div class="field">
            <label for="u-pass">{{ userDraft.id ? "New password (leave empty to keep)" : "Password (min. 8 chars)" }}</label>
            <input id="u-pass" v-model="userDraft.password" type="password" :required="!userDraft.id" minlength="8" autocomplete="new-password" />
          </div>
          <label style="display: flex; align-items: center; gap: 10px; color: var(--text)">
            <input v-model="userDraft.is_admin" type="checkbox" style="width: 16px; min-height: 0" />
            Administrator
          </label>
        </div>
        <div class="mdl-actions">
          <button type="button" class="btn ghost" @click="userDialogEl.close()">Cancel</button>
          <button type="submit" class="btn primary">Save</button>
        </div>
      </form>
    </dialog>

    <!-- Group dialog -->
    <dialog ref="groupDialogEl" class="mdl" @mousedown="modalPress" @click="modalBackdropClose" @close="groupDraft = null">
      <form v-if="groupDraft" @submit.prevent="saveGroup">
        <div class="mdl-head"><h2>{{ groupDraft.id ? "Edit group" : "New group" }}</h2></div>
        <div class="mdl-body">
          <div class="field">
            <label for="g-name">Group name</label>
            <input id="g-name" v-model="groupDraft.name" type="text" required />
          </div>
          <label style="margin-bottom: 6px">Members</label>
          <div class="member-list">
            <label v-for="u in users" :key="u.id">
              <input type="checkbox" :checked="groupDraft.members.has(u.id)" @change="toggleMember(u.id)" />
              {{ u.username }}
            </label>
          </div>
        </div>
        <div class="mdl-actions">
          <button type="button" class="btn ghost" @click="groupDialogEl.close()">Cancel</button>
          <button type="submit" class="btn primary">Save</button>
        </div>
      </form>
    </dialog>

    <div v-if="toast" class="toast" :class="toast.kind" role="status">{{ toast.message }}</div>
  </section>
</template>
