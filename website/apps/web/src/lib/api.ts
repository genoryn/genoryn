export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`/api/v1${path}`, { ...init, headers: { accept: 'application/json', ...(init?.headers ?? {}) } });
  const text = await res.text();
  let body: unknown = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = { error: text.slice(0, 200) };
  }
  if (!res.ok) throw new ApiError(res.status, (body as { error?: string })?.error ?? `HTTP ${res.status}`);
  return body as T;
}

export const apiUrl = (path: string) => `/api/v1${path}`;
