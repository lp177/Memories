<script setup>
// Application shell: brand bar with settings/admin/logout and the user avatar.
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { avatarUrl, fetchBlob, getMe, sessionToken, setSessionToken } from "./api.js";

const route = useRoute();
const router = useRouter();

const me = ref(null);
const avatarObjUrl = ref("");

const isOpenView = computed(() => route.name === "login" || route.name === "reset");

async function loadMe() {
  if (!sessionToken()) {
    me.value = null;
    return;
  }
  try {
    const data = await getMe();
    me.value = data.user;
    if (data.user.has_avatar) {
      const blob = await fetchBlob(avatarUrl(data.user.id));
      if (avatarObjUrl.value) URL.revokeObjectURL(avatarObjUrl.value);
      avatarObjUrl.value = URL.createObjectURL(blob);
    } else {
      avatarObjUrl.value = "";
    }
  } catch {
    me.value = null;
  }
}

function logOut() {
  setSessionToken(null);
  me.value = null;
  router.push({ name: "login" });
}

watch(
  () => route.name,
  (name, prev) => {
    // Refresh identity after login and after settings edits.
    if (prev === "login" || prev === "settings") loadMe();
  },
);

onMounted(loadMe);
</script>

<template>
  <header v-if="!isOpenView" class="topbar">
    <router-link to="/x/" class="brand" aria-label="Memories home">
      <svg width="26" height="26" viewBox="0 0 512 512" aria-hidden="true">
        <defs>
          <linearGradient id="brand-violet" x1="0" y1="0" x2="512" y2="512" gradientUnits="userSpaceOnUse">
            <stop offset="0" stop-color="#a78bfa" />
            <stop offset="1" stop-color="#6d5bd0" />
          </linearGradient>
        </defs>
        <rect width="512" height="512" rx="116" fill="url(#brand-violet)" />
        <path d="M120 176c0-18 14-32 32-32h84l30 34h126c18 0 32 14 32 32v126c0 18-14 32-32 32H152c-18 0-32-14-32-32V176z" fill="#171233" />
        <circle cx="256" cy="278" r="34" fill="#c9bcff" />
      </svg>
      <span class="wordmark">Memories</span>
    </router-link>

    <nav class="topbar-actions" aria-label="Account navigation">
      <router-link v-if="me && me.is_admin" to="/admin" class="btn ghost small">Admin</router-link>
      <router-link to="/settings" class="icon-btn" aria-label="Settings" title="Settings">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </router-link>
      <img v-if="avatarObjUrl" class="topbar-avatar" :src="avatarObjUrl" :alt="`Avatar of ${me?.username}`" />
      <button type="button" class="btn ghost small" @click="logOut">Log out</button>
    </nav>
  </header>

  <main :class="isOpenView ? '' : 'page'">
    <router-view />
  </main>
</template>
