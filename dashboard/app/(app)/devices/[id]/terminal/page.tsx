import { getDevice } from "../device-data";
import { Placeholder } from "../placeholder";
import { Terminal } from "../terminal";

export default async function TerminalPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  const d = await getDevice(id);
  if (d.status !== "Online") return <Placeholder device={d.id} />;
  return <Terminal device={d.id} />;
}
