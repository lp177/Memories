// Fetch wrapper for the Memories REST API. Attaches the Bearer session token
// from localStorage; on 401 (except login) clears it and redirects to /login.

const TOKEN_KEY = "memories_token";

class ApiError extends Error {
  constructor(message, status) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export function sessionToken() {
  return localStorage.getItem(TOKEN_KEY);
}

export function setSessionToken(token) {
  if (token) {
    localStorage.setItem(TOKEN_KEY, token);
  } else {
    localStorage.removeItem(TOKEN_KEY);
  }
}

async function errorDetail(response) {
  let detail = `Request failed (${response.status})`;
  try {
    const data = await response.json();
    if (typeof data.detail === "string") {
      detail = data.detail;
    }
  } catch {
    // not JSON
  }
  return detail;
}

async function request(path, options = {}) {
  const headers = { ...(options.headers || {}) };
  const token = sessionToken();
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }
  if (typeof options.body === "string") {
    headers["Content-Type"] = "application/json";
  }
  const response = await fetch(path, { ...options, headers });
  if (response.status === 401 && path !== "/api/login") {
    setSessionToken(null);
    window.location.hash = "#/login";
    throw new ApiError("Session expired — please sign in again.", 401);
  }
  if (!response.ok) {
    throw new ApiError(await errorDetail(response), response.status);
  }
  return response;
}

const jsonBody = (obj) => JSON.stringify(obj);

// ---- auth & profile ----
export const login = (username, password) =>
  request("/api/login", { method: "POST", body: jsonBody({ username, password }) }).then((r) => r.json());
export const forgot = (username_or_email) =>
  request("/api/forgot", { method: "POST", body: jsonBody({ username_or_email }) }).then((r) => r.json());
export const resetPassword = (token, password) =>
  request("/api/reset", { method: "POST", body: jsonBody({ token, password }) }).then((r) => r.json());
export const getMe = () => request("/api/me").then((r) => r.json());
export const updateMe = (changes) =>
  request("/api/me", { method: "PUT", body: jsonBody(changes) }).then((r) => r.json());
export const uploadAvatar = (file) => {
  const form = new FormData();
  form.append("file", file);
  return request("/api/me/avatar", { method: "POST", body: form }).then((r) => r.json());
};
export const deleteAvatar = () => request("/api/me/avatar", { method: "DELETE" }).then((r) => r.json());
export const avatarUrl = (userId) => `/api/users/${encodeURIComponent(userId)}/avatar`;
export const fetchBlob = (url) => request(url).then((r) => r.blob());
export const getDirectory = () => request("/api/directory").then((r) => r.json());

// ---- api tokens ----
export const getTokens = () => request("/api/tokens").then((r) => r.json());
export const createToken = (name, scope) =>
  request("/api/tokens", { method: "POST", body: jsonBody({ name, scope }) }).then((r) => r.json());
export const deleteToken = (id) =>
  request(`/api/tokens/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => r.json());

// ---- admin ----
export const adminUsers = () => request("/api/admin/users").then((r) => r.json());
export const adminCreateUser = (body) =>
  request("/api/admin/users", { method: "POST", body: jsonBody(body) }).then((r) => r.json());
export const adminUpdateUser = (id, body) =>
  request(`/api/admin/users/${encodeURIComponent(id)}`, { method: "PUT", body: jsonBody(body) }).then((r) => r.json());
export const adminDeleteUser = (id) =>
  request(`/api/admin/users/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => r.json());
export const adminResetLink = (id) =>
  request(`/api/admin/users/${encodeURIComponent(id)}/reset-link`, { method: "POST" }).then((r) => r.json());
export const adminGroups = () => request("/api/admin/groups").then((r) => r.json());
export const adminCreateGroup = (name, members) =>
  request("/api/admin/groups", { method: "POST", body: jsonBody({ name, members }) }).then((r) => r.json());
export const adminUpdateGroup = (id, body) =>
  request(`/api/admin/groups/${encodeURIComponent(id)}`, { method: "PUT", body: jsonBody(body) }).then((r) => r.json());
export const adminDeleteGroup = (id) =>
  request(`/api/admin/groups/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => r.json());

// ---- file system ----
export const fsList = (path) =>
  request(`/api/fs/list?path=${encodeURIComponent(path)}`).then((r) => r.json());
export const fsRead = (path) =>
  request(`/api/fs/file?path=${encodeURIComponent(path)}`).then((r) => r.json());
export const fsWrite = (path, content) =>
  request("/api/fs/file", { method: "PUT", body: jsonBody({ path, content }) }).then((r) => r.json());
export const fsMkdir = (path) =>
  request("/api/fs/mkdir", { method: "POST", body: jsonBody({ path }) }).then((r) => r.json());
export const fsDelete = (paths) =>
  request("/api/fs/delete", { method: "POST", body: jsonBody({ paths }) }).then((r) => r.json());
export const fsRename = (path, new_name) =>
  request("/api/fs/rename", { method: "POST", body: jsonBody({ path, new_name }) }).then((r) => r.json());
export const fsMove = (paths, dest) =>
  request("/api/fs/move", { method: "POST", body: jsonBody({ paths, dest }) }).then((r) => r.json());
export const fsCopy = (paths, dest) =>
  request("/api/fs/copy", { method: "POST", body: jsonBody({ paths, dest }) }).then((r) => r.json());

// ---- shares ----
export const sharesList = (path) =>
  request(`/api/shares?path=${encodeURIComponent(path)}`).then((r) => r.json());
export const sharesCreate = (body) =>
  request("/api/shares", { method: "POST", body: jsonBody(body) }).then((r) => r.json());
export const sharesDelete = (id) =>
  request(`/api/shares/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => r.json());

// ---- live events ----
/** EventSource for a project's live changes (token via query — EventSource
 *  cannot set headers). Caller must close() it. */
export function eventStream(project) {
  const token = sessionToken();
  return new EventSource(
    `/api/events/stream?project=${encodeURIComponent(project)}&access_token=${encodeURIComponent(token || "")}`,
  );
}
