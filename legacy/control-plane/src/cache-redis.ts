import { RedisClient } from "bun";
import { ViewCache } from "./cache";
export function connectViewCache(url: string) {
  const redis = new RedisClient(url, {
    connectionTimeout: 100,
    autoReconnect: false,
    maxRetries: 0,
    enableOfflineQueue: false,
  });
  return {
    cache: new ViewCache({
      get: async (key) => {
        // Offline queuing is disabled: establish the connection before GET.
        // The view wrapper bounds this wait and falls back to its loader.
        if (!redis.connected) await redis.connect();
        return redis.get(key);
      },
      set: (key, value, ttlSeconds) =>
        redis.send("SET", [key, value, "EX", String(ttlSeconds)]),
    }),
    close: () => redis.close(),
  };
}
