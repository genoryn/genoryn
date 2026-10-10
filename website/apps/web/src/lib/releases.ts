import { createResource } from 'solid-js';
import { isServer } from 'solid-js/web';
import type { Release } from '@genoryn/shared';
import { api } from './api';

export interface ReleasesResponse {
  releases: Release[];
  repo: { stars?: number; forks?: number; watchers?: number; open_issues?: number; pushed_at?: string; license?: string; topics?: string[] } | null;
  cached_at: number | null;
  error?: string;
}

let shared: Promise<ReleasesResponse> | null = null;
export function fetchReleases(): Promise<ReleasesResponse> {
  shared ??= api<ReleasesResponse>('/releases').catch((e) => {
    shared = null;
    throw e;
  });
  return shared;
}

export function useReleases() {
  return createResource(() => (isServer ? null : true), () => fetchReleases());
}
