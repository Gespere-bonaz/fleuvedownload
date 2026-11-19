export type Status =
  | "queued"
  | "scheduled"
  | "downloading"
  | "assembling"
  | "paused"
  | "completed"
  | "failed";

export type Kind = "file" | "hls" | "site";

export interface Segment {
  start: number;
  end: number;
  pos: number;
  active: boolean;
}

export interface DownloadView {
  id: string;
  url: string;
  kind: Kind;
  fileName: string;
  destDir: string;
  category: string;
  status: Status;
  total: number | null;
  received: number;
  resumable: boolean;
  progress: number | null;
  speed: number;
  etaSecs: number | null;
  connections: number;
  segments: Segment[];
  hlsTotalParts: number;
  hlsDoneParts: number;
  createdAt: number;
  scheduledAt: number | null;
  completedAt: number | null;
  error: string | null;
  /** Hauteur d'image maximale choisie (720 = 720p) ; null = la meilleure. */
  maxHeight: number | null;
  /** Vidéo en attente du choix de qualité. */
  qualityPending: boolean;
}

export interface QualityOption {
  height: number;
  size: number | null;
}

export interface RequestHeaders {
  referer?: string | null;
  cookie?: string | null;
  userAgent?: string | null;
}

export interface AddRequest {
  url: string;
  fileName?: string | null;
  destDir?: string | null;
  headers?: RequestHeaders;
  scheduledAt?: number | null;
  startPaused?: boolean;
  kind?: Kind | null;
  /** « best » ou une hauteur (« 720 ») ; absent : selon les réglages. */
  quality?: string | null;
}

export interface Settings {
  downloadDir: string;
  connections: number;
  maxConcurrent: number;
  speedLimitKbps: number;
  sortByCategory: boolean;
  extensionToken: string;
  serverPort: number;
  /** « ask », « best » ou une hauteur maximale (« 1080 »). */
  videoQuality: string;
}
