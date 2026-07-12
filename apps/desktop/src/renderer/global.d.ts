/// <reference types="vite/client" />

export {};

declare global {
  interface Window {
    generalAgent?: {
      ownerPolicy(): Promise<unknown>;
      ownerRequest(path: string, init: RequestInit): Promise<unknown>;
    };
  }
}
