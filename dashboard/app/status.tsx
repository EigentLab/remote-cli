import type { DeviceStatus } from "@/lib/api";
import { cn } from "@/lib/utils";

// A square signal, as on a line map: cobalt is live, a hollow square is out of service.
const TONE: Record<DeviceStatus, string> = {
  Online: "border-ok bg-ok",
  Offline: "border-muted-foreground bg-transparent",
};

export function Status({ status }: { status: DeviceStatus }) {
  return (
    <span className="inline-flex items-center gap-2 whitespace-nowrap text-sm font-medium">
      <span aria-hidden className={cn("size-2.5 shrink-0 border-2", TONE[status])} />
      {status}
    </span>
  );
}
