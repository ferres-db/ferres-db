import { useState, useCallback, useRef, useEffect } from "react";
import { getWsUrl } from "@/api/ferresdb";
import type {
  WsClientMessage,
  WsServerMessage,
  WsLogEntry,
} from "@/types";

export type WsStatus = "disconnected" | "connecting" | "connected" | "error";

let logIdCounter = 0;
function nextLogId(): string {
  return `ws-${Date.now()}-${++logIdCounter}`;
}

export function useWebSocket() {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pingIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const [status, setStatus] = useState<WsStatus>("disconnected");
  const [error, setError] = useState<string | null>(null);
  const [messages, setMessages] = useState<WsLogEntry[]>([]);
  const [sentCount, setSentCount] = useState(0);
  const [receivedCount, setReceivedCount] = useState(0);
  const [connectedAt, setConnectedAt] = useState<Date | null>(null);

  const clearMessages = useCallback(() => {
    setMessages([]);
    setSentCount(0);
    setReceivedCount(0);
  }, []);

  const addLog = useCallback(
    (direction: "sent" | "received", message: WsClientMessage | WsServerMessage, raw?: string) => {
      const entry: WsLogEntry = {
        id: nextLogId(),
        timestamp: new Date(),
        direction,
        message,
        raw,
      };
      setMessages((prev) => {
        const next = [...prev, entry];
        // Keep last 500 messages to avoid memory issues
        return next.length > 500 ? next.slice(-500) : next;
      });
      if (direction === "sent") setSentCount((c) => c + 1);
      else setReceivedCount((c) => c + 1);
    },
    [],
  );

  const disconnect = useCallback(() => {
    if (reconnectTimerRef.current) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
    if (pingIntervalRef.current) {
      clearInterval(pingIntervalRef.current);
      pingIntervalRef.current = null;
    }
    if (wsRef.current) {
      wsRef.current.close(1000, "User disconnected");
      wsRef.current = null;
    }
    setStatus("disconnected");
    setConnectedAt(null);
  }, []);

  const connect = useCallback(
    (token?: string) => {
      // Clean up existing connection
      disconnect();

      setStatus("connecting");
      setError(null);

      const url = getWsUrl(token);
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        setStatus("connected");
        setConnectedAt(new Date());
        setError(null);

        // Start heartbeat: send ping every 25s
        pingIntervalRef.current = setInterval(() => {
          if (ws.readyState === WebSocket.OPEN) {
            const ping: WsClientMessage = { type: "ping" };
            ws.send(JSON.stringify(ping));
            addLog("sent", ping);
          }
        }, 25_000);
      };

      ws.onmessage = (event) => {
        try {
          const data: WsServerMessage = JSON.parse(event.data);
          addLog("received", data, event.data);
        } catch {
          // Non-JSON message
          addLog("received", { type: "error", message: event.data, code: 0 }, event.data);
        }
      };

      ws.onerror = () => {
        setStatus("error");
        setError("WebSocket connection error");
      };

      ws.onclose = (event) => {
        if (pingIntervalRef.current) {
          clearInterval(pingIntervalRef.current);
          pingIntervalRef.current = null;
        }

        if (event.code !== 1000) {
          // Abnormal close — try reconnect after 3s
          setStatus("error");
          setError(`Connection closed: ${event.reason || `code ${event.code}`}`);
          reconnectTimerRef.current = setTimeout(() => {
            if (wsRef.current === ws) {
              connect(token);
            }
          }, 3_000);
        } else {
          setStatus("disconnected");
          setConnectedAt(null);
        }
      };
    },
    [disconnect, addLog],
  );

  const send = useCallback(
    (message: WsClientMessage) => {
      if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) {
        setError("WebSocket is not connected");
        return;
      }
      const raw = JSON.stringify(message);
      wsRef.current.send(raw);
      addLog("sent", message, raw);
    },
    [addLog],
  );

  const sendUpsert = useCallback(
    (
      collection: string,
      points: Array<{ id: string; vector: number[]; metadata?: Record<string, unknown> }>,
    ) => {
      send({ type: "upsert", collection, points });
    },
    [send],
  );

  const sendSubscribe = useCallback(
    (collection: string, events?: Array<"upsert" | "delete">) => {
      send({ type: "subscribe", collection, events });
    },
    [send],
  );

  const sendPing = useCallback(() => {
    send({ type: "ping" });
  }, [send]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (reconnectTimerRef.current) clearTimeout(reconnectTimerRef.current);
      if (pingIntervalRef.current) clearInterval(pingIntervalRef.current);
      if (wsRef.current) {
        wsRef.current.close(1000);
        wsRef.current = null;
      }
    };
  }, []);

  return {
    status,
    error,
    messages,
    sentCount,
    receivedCount,
    connectedAt,
    connect,
    disconnect,
    send,
    sendUpsert,
    sendSubscribe,
    sendPing,
    clearMessages,
  };
}
