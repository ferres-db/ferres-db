import { useState, useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { getStoredRole } from "@/api/ferresdb";
import { useWebSocket, type WsStatus } from "@/hooks/useWebSocket";
import { useCollections } from "@/hooks/useCollections";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { Badge } from "@/components/ui/Badge";
import { Skeleton } from "@/components/ui/Skeleton";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/Tabs";
import {
  Radio,
  Wifi,
  WifiOff,
  Send,
  Trash2,
  Bell,
  ArrowUp,
  ArrowDown,
  Clock,
  Zap,
  AlertCircle,
} from "lucide-react";
import type { WsLogEntry, WsServerMessage } from "@/types";

const STATUS_CONFIG: Record<WsStatus, { color: string; label: string; dotColor: string }> = {
  disconnected: { color: "text-gray-400", label: "Disconnected", dotColor: "bg-gray-400" },
  connecting: { color: "text-yellow-400", label: "Connecting...", dotColor: "bg-yellow-400" },
  connected: { color: "text-green-400", label: "Connected", dotColor: "bg-green-400" },
  error: { color: "text-red-400", label: "Error", dotColor: "bg-red-400" },
};

export const Streaming = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role === "viewer") navigate("/", { replace: true });
  }, [role, navigate]);

  const ws = useWebSocket();
  const [operationTab, setOperationTab] = useState("upsert");

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold text-gray-50">WebSocket Console</h1>
        <p className="text-gray-400 mt-2">
          Real-time streaming — ingest data and subscribe to collection events via WebSocket
        </p>
      </div>

      <div className="grid grid-cols-1 xl:grid-cols-3 gap-6">
        {/* Left column: Connection + Operations */}
        <div className="xl:col-span-1 space-y-6">
          <ConnectionPanel ws={ws} />

          {ws.status === "connected" && (
            <Card>
              <CardHeader>
                <CardTitle className="flex items-center gap-2">
                  <Zap className="h-5 w-5" />
                  Operations
                </CardTitle>
              </CardHeader>
              <CardContent>
                <Tabs value={operationTab} onValueChange={setOperationTab}>
                  <TabsList className="w-full">
                    <TabsTrigger value="upsert">Upsert</TabsTrigger>
                    <TabsTrigger value="subscribe">Subscribe</TabsTrigger>
                  </TabsList>
                  <TabsContent value="upsert">
                    <UpsertPanel ws={ws} />
                  </TabsContent>
                  <TabsContent value="subscribe">
                    <SubscribePanel ws={ws} />
                  </TabsContent>
                </Tabs>
              </CardContent>
            </Card>
          )}
        </div>

        {/* Right column: Live Feed + Metrics */}
        <div className="xl:col-span-2 space-y-6">
          <MetricsBar ws={ws} />
          <LiveFeed messages={ws.messages} onClear={ws.clearMessages} />
        </div>
      </div>
    </div>
  );
};

// ─── Connection Panel ─────────────────────────────────────────────────

function ConnectionPanel({
  ws,
}: {
  ws: ReturnType<typeof useWebSocket>;
}) {
  const [token, setToken] = useState("");
  const cfg = STATUS_CONFIG[ws.status];

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Radio className="h-5 w-5" />
          Connection
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Status */}
        <div className="flex items-center gap-3">
          <div className={`h-3 w-3 rounded-full ${cfg.dotColor} ${ws.status === "connecting" ? "animate-pulse" : ""}`} />
          <span className={`text-sm font-medium ${cfg.color}`}>{cfg.label}</span>
          {ws.connectedAt && (
            <span className="text-xs text-gray-500 ml-auto">
              since {ws.connectedAt.toLocaleTimeString()}
            </span>
          )}
        </div>

        {ws.error && (
          <div className="flex items-center gap-2 p-2 rounded-md bg-red-500/10 border border-red-500/50">
            <AlertCircle className="h-4 w-4 text-red-500 shrink-0" />
            <p className="text-xs text-red-400">{ws.error}</p>
          </div>
        )}

        {/* Token */}
        {ws.status === "disconnected" || ws.status === "error" ? (
          <>
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Token (optional, uses session if empty)
              </label>
              <Input
                type="password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="sk-xxx or leave empty"
                className="bg-bg-secondary border-bg-tertiary text-gray-50"
              />
            </div>
            <Button
              onClick={() => ws.connect(token || undefined)}
              className="w-full"
              variant="primary"
            >
              <Wifi className="h-4 w-4 mr-2" />
              Connect
            </Button>
          </>
        ) : (
          <Button onClick={ws.disconnect} className="w-full" variant="danger">
            <WifiOff className="h-4 w-4 mr-2" />
            Disconnect
          </Button>
        )}
      </CardContent>
    </Card>
  );
}

// ─── Metrics Bar ──────────────────────────────────────────────────────

function MetricsBar({ ws }: { ws: ReturnType<typeof useWebSocket> }) {
  const [uptime, setUptime] = useState("");

  useEffect(() => {
    if (!ws.connectedAt) {
      setUptime("-");
      return;
    }
    const interval = setInterval(() => {
      const diffSec = Math.floor((Date.now() - ws.connectedAt!.getTime()) / 1000);
      const m = Math.floor(diffSec / 60);
      const s = diffSec % 60;
      setUptime(`${m}m ${s}s`);
    }, 1000);
    return () => clearInterval(interval);
  }, [ws.connectedAt]);

  return (
    <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
      <div className="bg-bg-secondary rounded-lg px-4 py-3 border border-bg-tertiary">
        <div className="flex items-center gap-2 text-gray-400 mb-1">
          <ArrowUp className="h-3.5 w-3.5" />
          <p className="text-xs">Sent</p>
        </div>
        <p className="text-lg font-semibold text-gray-50">{ws.sentCount}</p>
      </div>
      <div className="bg-bg-secondary rounded-lg px-4 py-3 border border-bg-tertiary">
        <div className="flex items-center gap-2 text-gray-400 mb-1">
          <ArrowDown className="h-3.5 w-3.5" />
          <p className="text-xs">Received</p>
        </div>
        <p className="text-lg font-semibold text-gray-50">{ws.receivedCount}</p>
      </div>
      <div className="bg-bg-secondary rounded-lg px-4 py-3 border border-bg-tertiary">
        <div className="flex items-center gap-2 text-gray-400 mb-1">
          <Clock className="h-3.5 w-3.5" />
          <p className="text-xs">Uptime</p>
        </div>
        <p className="text-lg font-semibold text-gray-50">{uptime}</p>
      </div>
      <div className="bg-bg-secondary rounded-lg px-4 py-3 border border-bg-tertiary">
        <div className="flex items-center gap-2 text-gray-400 mb-1">
          <Radio className="h-3.5 w-3.5" />
          <p className="text-xs">Status</p>
        </div>
        <p className={`text-lg font-semibold ${STATUS_CONFIG[ws.status].color}`}>
          {STATUS_CONFIG[ws.status].label}
        </p>
      </div>
    </div>
  );
}

// ─── Upsert Panel ─────────────────────────────────────────────────────

function UpsertPanel({ ws }: { ws: ReturnType<typeof useWebSocket> }) {
  const { data: collections, isLoading } = useCollections();
  const [collection, setCollection] = useState("");
  const [pointsJson, setPointsJson] = useState(
    JSON.stringify(
      [{ id: "point-1", vector: [0.1, 0.2, 0.3], metadata: { text: "example" } }],
      null,
      2,
    ),
  );
  const [jsonError, setJsonError] = useState<string | null>(null);

  const handleSend = () => {
    if (!collection) return;
    setJsonError(null);
    try {
      const points = JSON.parse(pointsJson);
      if (!Array.isArray(points)) throw new Error("Points must be a JSON array");
      ws.sendUpsert(collection, points);
    } catch (err) {
      setJsonError(err instanceof Error ? err.message : "Invalid JSON");
    }
  };

  return (
    <div className="space-y-4 mt-3">
      <div>
        <label className="block text-sm font-medium text-gray-400 mb-2">Collection</label>
        {isLoading ? (
          <Skeleton className="h-10 w-full" />
        ) : (
          <select
            value={collection}
            onChange={(e) => setCollection(e.target.value)}
            className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
          >
            <option value="">Select collection</option>
            {Array.isArray(collections) &&
              collections.map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name}
                </option>
              ))}
          </select>
        )}
      </div>

      <div>
        <label className="block text-sm font-medium text-gray-400 mb-2">Points (JSON)</label>
        <textarea
          value={pointsJson}
          onChange={(e) => setPointsJson(e.target.value)}
          rows={8}
          className="w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 font-mono focus:outline-none focus:ring-2 focus:ring-orange-500"
        />
      </div>

      {jsonError && (
        <div className="flex items-center gap-2 p-2 rounded-md bg-red-500/10 border border-red-500/50">
          <AlertCircle className="h-4 w-4 text-red-500 shrink-0" />
          <p className="text-xs text-red-400">{jsonError}</p>
        </div>
      )}

      <Button onClick={handleSend} disabled={!collection} className="w-full" variant="primary">
        <Send className="h-4 w-4 mr-2" />
        Send Upsert
      </Button>
    </div>
  );
}

// ─── Subscribe Panel ──────────────────────────────────────────────────

function SubscribePanel({ ws }: { ws: ReturnType<typeof useWebSocket> }) {
  const { data: collections, isLoading } = useCollections();
  const [collection, setCollection] = useState("");
  const [events, setEvents] = useState<{ upsert: boolean; delete: boolean }>({
    upsert: true,
    delete: true,
  });

  const handleSubscribe = () => {
    if (!collection) return;
    const eventFilter: Array<"upsert" | "delete"> = [];
    if (events.upsert) eventFilter.push("upsert");
    if (events.delete) eventFilter.push("delete");
    ws.sendSubscribe(collection, eventFilter.length > 0 ? eventFilter : undefined);
  };

  return (
    <div className="space-y-4 mt-3">
      <div>
        <label className="block text-sm font-medium text-gray-400 mb-2">Collection</label>
        {isLoading ? (
          <Skeleton className="h-10 w-full" />
        ) : (
          <select
            value={collection}
            onChange={(e) => setCollection(e.target.value)}
            className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
          >
            <option value="">Select collection</option>
            {Array.isArray(collections) &&
              collections.map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name}
                </option>
              ))}
          </select>
        )}
      </div>

      <div>
        <label className="block text-sm font-medium text-gray-400 mb-2">Event Filters</label>
        <div className="flex gap-4">
          <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
            <input
              type="checkbox"
              checked={events.upsert}
              onChange={(e) => setEvents((prev) => ({ ...prev, upsert: e.target.checked }))}
              className="rounded border-bg-tertiary bg-bg-secondary text-orange-500 focus:ring-orange-500"
            />
            Upsert
          </label>
          <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
            <input
              type="checkbox"
              checked={events.delete}
              onChange={(e) => setEvents((prev) => ({ ...prev, delete: e.target.checked }))}
              className="rounded border-bg-tertiary bg-bg-secondary text-orange-500 focus:ring-orange-500"
            />
            Delete
          </label>
        </div>
      </div>

      <Button
        onClick={handleSubscribe}
        disabled={!collection}
        className="w-full"
        variant="primary"
      >
        <Bell className="h-4 w-4 mr-2" />
        Subscribe
      </Button>
    </div>
  );
}

// ─── Live Feed ────────────────────────────────────────────────────────

function getMessageColor(entry: WsLogEntry): string {
  if (entry.direction === "sent") return "border-l-blue-500";
  const msg = entry.message as WsServerMessage;
  switch (msg.type) {
    case "ack":
      return "border-l-green-500";
    case "event":
      return "border-l-cyan-500";
    case "error":
      return "border-l-red-500";
    case "pong":
      return "border-l-gray-500";
    default:
      return "border-l-gray-500";
  }
}

function getMessageLabel(entry: WsLogEntry): { text: string; variant: "default" | "success" | "warning" | "danger" } {
  if (entry.direction === "sent") {
    return { text: `SENT: ${(entry.message as any).type}`, variant: "default" };
  }
  const msg = entry.message as WsServerMessage;
  switch (msg.type) {
    case "ack":
      return { text: `ACK (${msg.upserted} ok, ${msg.failed} fail)`, variant: "success" };
    case "event":
      return { text: `EVENT: ${msg.action} on ${msg.collection}`, variant: "warning" };
    case "error":
      return { text: `ERROR (${msg.code})`, variant: "danger" };
    case "pong":
      return { text: "PONG", variant: "default" };
    default:
      return { text: (msg as any).type, variant: "default" };
  }
}

function LiveFeed({
  messages,
  onClear,
}: {
  messages: WsLogEntry[];
  onClear: () => void;
}) {
  const feedRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  useEffect(() => {
    if (autoScroll && feedRef.current) {
      feedRef.current.scrollTop = feedRef.current.scrollHeight;
    }
  }, [messages, autoScroll]);

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between">
          <CardTitle className="flex items-center gap-2">
            <Radio className="h-5 w-5" />
            Live Feed
            <span className="text-sm font-normal text-gray-400">({messages.length} messages)</span>
          </CardTitle>
          <div className="flex items-center gap-2">
            <label className="flex items-center gap-1.5 text-xs text-gray-400 cursor-pointer">
              <input
                type="checkbox"
                checked={autoScroll}
                onChange={(e) => setAutoScroll(e.target.checked)}
                className="rounded border-bg-tertiary bg-bg-secondary text-orange-500 focus:ring-orange-500 h-3 w-3"
              />
              Auto-scroll
            </label>
            <Button variant="secondary" size="sm" onClick={onClear}>
              <Trash2 className="h-3.5 w-3.5 mr-1" />
              Clear
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <div
          ref={feedRef}
          className="h-[500px] overflow-y-auto space-y-1 bg-bg-primary rounded-lg p-2"
        >
          {messages.length === 0 ? (
            <div className="flex items-center justify-center h-full">
              <p className="text-gray-500 text-sm">
                No messages yet. Connect and send operations to see live data.
              </p>
            </div>
          ) : (
            messages.map((entry) => {
              const label = getMessageLabel(entry);
              return (
                <div
                  key={entry.id}
                  className={`border-l-2 ${getMessageColor(entry)} bg-bg-secondary rounded-r-md px-3 py-2`}
                >
                  <div className="flex items-center gap-2 mb-1">
                    {entry.direction === "sent" ? (
                      <ArrowUp className="h-3 w-3 text-blue-400" />
                    ) : (
                      <ArrowDown className="h-3 w-3 text-green-400" />
                    )}
                    <Badge variant={label.variant} className="text-[10px] px-1.5 py-0">
                      {label.text}
                    </Badge>
                    <span className="text-[10px] text-gray-500 ml-auto">
                      {entry.timestamp.toLocaleTimeString()}
                    </span>
                  </div>
                  <pre className="text-xs text-gray-400 font-mono overflow-x-auto whitespace-pre-wrap">
                    {JSON.stringify(entry.message, null, 2)}
                  </pre>
                </div>
              );
            })
          )}
        </div>
      </CardContent>
    </Card>
  );
}
