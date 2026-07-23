<script setup>
// Choose a new password from a reset link (/#/reset?token=...).
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { resetPassword } from "../api.js";

const route = useRoute();
const router = useRouter();
const password = ref("");
const confirm = ref("");
const error = ref("");
const busy = ref(false);

async function submit() {
  error.value = "";
  if (password.value !== confirm.value) {
    error.value = "Passwords do not match.";
    return;
  }
  busy.value = true;
  try {
    await resetPassword(String(route.query.token || ""), password.value);
    router.push({ name: "login" });
  } catch (e) {
    error.value = (e && e.message) || "Reset failed.";
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="login-wrap">
    <form class="login-card" @submit.prevent="submit">
      <h1>Choose a new password</h1>
      <p v-if="error" class="alert" role="alert">{{ error }}</p>
      <div class="field">
        <label for="np">New password (min. 8 characters)</label>
        <input id="np" v-model="password" type="password" autocomplete="new-password" minlength="8" required />
      </div>
      <div class="field">
        <label for="np2">Confirm password</label>
        <input id="np2" v-model="confirm" type="password" autocomplete="new-password" minlength="8" required />
      </div>
      <button type="submit" class="btn primary btn-block" :disabled="busy">
        {{ busy ? "…" : "Set password" }}
      </button>
    </form>
  </div>
</template>
