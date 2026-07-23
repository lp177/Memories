<script setup>
// Per-user settings: profile (pseudo, e-mail, avatar), password change, and
// API tokens for LLM access to the REST API & MCP (create / copy / revoke).
import { onMounted, ref } from "vue";
import {
  avatarUrl,
  createToken,
  deleteAvatar,
  deleteToken,
  fetchBlob,
  getMe,
  getTokens,
  updateMe,
  uploadAvatar,
} from "../api.js";

const me = ref(null);
const avatarObjUrl = ref("");
const usernameDraft = ref("");
const emailDraft = ref("");
const currentPassword = ref("");
const newPassword = ref("");
const fileInput = ref(null);

const tokens = ref([]);
const newTokenName = ref("");
const newTokenScope = ref("read");
const createdToken = ref(null);
const copied = ref(false);

const toast = ref(null);
let toastTimer = null;
function showToast(message, kind) {
  toast.value = { message, kind };
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.value = null), 3200);
}

async function load() {
  const data = await getMe();
  me.value = data.user;
  usernameDraft.value = data.user.username;
  emailDraft.value = data.user.email;
  await loadAvatar();
  tokens.value = (await getTokens()).tokens;
}

async function loadAvatar() {
  if (avatarObjUrl.value) URL.revokeObjectURL(avatarObjUrl.value);
  avatarObjUrl.value = "";
  if (me.value?.has_avatar) {
    try {
      const blob = await fetchBlob(avatarUrl(me.value.id));
      avatarObjUrl.value = URL.createObjectURL(blob);
    } catch {
      avatarObjUrl.value = "";
    }
  }
}

async function saveProfile() {
  try {
    const res = await updateMe({ username: usernameDraft.value, email: emailDraft.value });
    me.value = res.user;
    showToast("Profile saved.", "success");
  } catch (e) {
    showToast((e && e.message) || "Failed to save the profile.", "error");
  }
}

async function changePassword() {
  try {
    await updateMe({ password: newPassword.value, current_password: currentPassword.value });
    currentPassword.value = "";
    newPassword.value = "";
    showToast("Password changed.", "success");
  } catch (e) {
    showToast((e && e.message) || "Failed to change the password.", "error");
  }
}

async function onAvatarPick(event) {
  const file = event.target.files && event.target.files[0];
  if (!file) return;
  try {
    await uploadAvatar(file);
    me.value.has_avatar = true;
    await loadAvatar();
    showToast("Avatar updated.", "success");
  } catch (e) {
    showToast((e && e.message) || "Avatar upload failed.", "error");
  } finally {
    if (fileInput.value) fileInput.value.value = "";
  }
}

async function removeAvatar() {
  try {
    await deleteAvatar();
    me.value.has_avatar = false;
    await loadAvatar();
  } catch (e) {
    showToast((e && e.message) || "Failed to remove the avatar.", "error");
  }
}

async function doCreateToken() {
  const name = newTokenName.value.trim();
  if (!name) {
    showToast("Give the token a name first.", "error");
    return;
  }
  try {
    createdToken.value = await createToken(name, newTokenScope.value);
    copied.value = false;
    newTokenName.value = "";
    tokens.value = (await getTokens()).tokens;
  } catch (e) {
    showToast((e && e.message) || "Failed to create the token.", "error");
  }
}

async function copyToken() {
  try {
    await navigator.clipboard.writeText(createdToken.value.token);
    copied.value = true;
  } catch {
    showToast("Copy failed — select the token and copy it manually.", "error");
  }
}

async function removeToken(t) {
  if (!window.confirm(`Delete API token "${t.name}"? Any LLM/client using it will lose access.`)) return;
  try {
    await deleteToken(t.id);
    tokens.value = (await getTokens()).tokens;
    if (createdToken.value?.api_token?.id === t.id) createdToken.value = null;
  } catch (e) {
    showToast((e && e.message) || "Failed to delete the token.", "error");
  }
}

function fmtDate(iso) {
  return iso ? new Date(iso).toLocaleString() : "never";
}

onMounted(load);
</script>

<template>
  <section>
    <h1 style="margin-top: 8px">Settings</h1>

    <section v-if="me" class="panel">
      <h2>Profile</h2>
      <div class="avatar-row">
        <img v-if="avatarObjUrl" class="avatar-lg" :src="avatarObjUrl" :alt="`Avatar of ${me.username}`" />
        <div v-else class="avatar-lg avatar-ph" aria-hidden="true">
          {{ (me.username || "?").slice(0, 2).toUpperCase() }}
        </div>
        <div style="display: flex; flex-direction: column; gap: 8px">
          <input ref="fileInput" type="file" accept="image/*" aria-label="Choose an avatar image" @change="onAvatarPick" />
          <button v-if="me.has_avatar" type="button" class="btn ghost small" @click="removeAvatar">Remove avatar</button>
        </div>
      </div>
      <div class="field" style="margin-top: 16px; max-width: 420px">
        <label for="p-user">Pseudo (username)</label>
        <input id="p-user" v-model="usernameDraft" type="text" />
        <p class="field-hint">Also the name of your personal folder inside each project.</p>
      </div>
      <div class="field" style="max-width: 420px">
        <label for="p-mail">E-mail</label>
        <input id="p-mail" v-model="emailDraft" type="email" />
      </div>
      <button type="button" class="btn primary" @click="saveProfile">Save profile</button>
    </section>

    <section class="panel">
      <h2>Change password</h2>
      <div class="field" style="max-width: 420px">
        <label for="pw-cur">Current password</label>
        <input id="pw-cur" v-model="currentPassword" type="password" autocomplete="current-password" />
      </div>
      <div class="field" style="max-width: 420px">
        <label for="pw-new">New password (min. 8 characters)</label>
        <input id="pw-new" v-model="newPassword" type="password" autocomplete="new-password" minlength="8" />
      </div>
      <button type="button" class="btn primary" :disabled="!currentPassword || newPassword.length < 8" @click="changePassword">
        Change password
      </button>
    </section>

    <section class="panel">
      <h2>API tokens</h2>
      <p class="field-hint" style="margin: 0 0 14px">
        Give an LLM (or any client) access to the REST API and the MCP endpoint with
        <code>Authorization: Bearer &lt;token&gt;</code>. Read &amp; write scope is needed to create or edit memories.
      </p>
      <div style="display: flex; gap: 10px; flex-wrap: wrap; align-items: center; margin-bottom: 14px">
        <input
          v-model="newTokenName"
          type="text"
          placeholder="Token name (e.g. claude-code)"
          aria-label="New token name"
          style="flex: 1; min-width: 220px; width: auto"
          @keyup.enter="doCreateToken"
        />
        <select v-model="newTokenScope" aria-label="New token scope" style="width: auto; min-width: 150px">
          <option value="read">Read only</option>
          <option value="readwrite">Read &amp; write</option>
        </select>
        <button type="button" class="btn primary" @click="doCreateToken">Create token</button>
      </div>

      <div v-if="createdToken" class="token-reveal">
        <strong>Copy your new token now</strong> — it is stored hashed and will never be shown again.
        <div class="token-reveal-row">
          <input class="token-value" :value="createdToken.token" readonly aria-label="New API token" @focus="$event.target.select()" />
          <button type="button" class="btn" @click="copyToken">{{ copied ? "Copied ✓" : "Copy" }}</button>
          <button type="button" class="btn ghost" @click="createdToken = null">Done</button>
        </div>
      </div>

      <div v-if="tokens.length" class="list" role="table" aria-label="API tokens">
        <div class="list-head" role="row">
          <span role="columnheader" style="flex: 0 0 140px">Name</span>
          <span role="columnheader" style="flex: 0 0 110px">Token</span>
          <span role="columnheader" style="flex: 0 0 90px">Scope</span>
          <span role="columnheader" class="grow">Last used</span>
        </div>
        <div v-for="t in tokens" :key="t.id" class="list-row" role="row">
          <span role="cell" style="flex: 0 0 140px; font-weight: 600">{{ t.name }}</span>
          <span role="cell" style="flex: 0 0 110px"><code>{{ t.token_prefix }}…</code></span>
          <span role="cell" style="flex: 0 0 90px">
            <span class="chip" :class="t.scope === 'readwrite' ? 'rw' : 'ro'">{{ t.scope }}</span>
          </span>
          <span role="cell" class="grow muted">{{ fmtDate(t.last_used_at) }}</span>
          <span role="cell" class="btn-group">
            <button type="button" class="btn danger-btn small" @click="removeToken(t)">Delete</button>
          </span>
        </div>
      </div>
      <p v-else class="muted">No API tokens yet — create one above.</p>
    </section>

    <div v-if="toast" class="toast" :class="toast.kind" role="status">{{ toast.message }}</div>
  </section>
</template>
