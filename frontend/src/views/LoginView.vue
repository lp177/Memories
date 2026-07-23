<script setup>
// Login form + "forgot password" flow.
import { ref } from "vue";
import { useRouter } from "vue-router";
import { forgot, login, setSessionToken } from "../api.js";

const router = useRouter();
const username = ref("");
const password = ref("");
const error = ref("");
const busy = ref(false);
const forgotMode = ref(false);
const forgotSent = ref(false);

async function submit() {
  error.value = "";
  busy.value = true;
  try {
    if (forgotMode.value) {
      await forgot(username.value.trim());
      forgotSent.value = true;
    } else {
      const data = await login(username.value.trim(), password.value);
      setSessionToken(data.token);
      router.push("/x/");
    }
  } catch (e) {
    error.value = (e && e.message) || "Sign-in failed.";
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="login-wrap">
    <form class="login-card" @submit.prevent="submit">
      <div class="login-brand">
        <svg viewBox="0 0 512 512" aria-hidden="true">
          <rect width="512" height="512" rx="116" fill="#a78bfa" />
          <path d="M120 176c0-18 14-32 32-32h84l30 34h126c18 0 32 14 32 32v126c0 18-14 32-32 32H152c-18 0-32-14-32-32V176z" fill="#171233" />
          <circle cx="256" cy="278" r="34" fill="#c9bcff" />
        </svg>
        <div>
          <h1>Memories</h1>
          <p class="muted" style="margin: 0; font-size: 13px">Shared memory store for LLM workflows</p>
        </div>
      </div>

      <p v-if="error" class="alert" role="alert">{{ error }}</p>
      <p v-if="forgotSent" class="alert info" role="status">
        If the account exists, a reset link has been sent (or handed to your admin).
      </p>

      <div class="field">
        <label for="login-user">{{ forgotMode ? "Username or e-mail" : "Username" }}</label>
        <input id="login-user" v-model="username" type="text" autocomplete="username" required />
      </div>
      <div v-if="!forgotMode" class="field">
        <label for="login-pass">Password</label>
        <input id="login-pass" v-model="password" type="password" autocomplete="current-password" required />
      </div>

      <button type="submit" class="btn primary btn-block" :disabled="busy">
        {{ busy ? "…" : forgotMode ? "Send reset link" : "Sign in" }}
      </button>
      <p style="text-align: center; margin: 14px 0 0">
        <a href="#" @click.prevent="forgotMode = !forgotMode; forgotSent = false">
          {{ forgotMode ? "← Back to sign in" : "Forgot password?" }}
        </a>
      </p>
    </form>
  </div>
</template>
