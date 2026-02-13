import { Terminal } from "./Terminal";
import { InputBar } from "./InputBar";
import type { WshClient } from "../api/ws";

interface SessionPaneProps {
  session: string;
  client: WshClient;
}

export function SessionPane({ session, client }: SessionPaneProps) {
  return (
    <div class="session-pane">
      <Terminal session={session} />
      <InputBar session={session} client={client} />
    </div>
  );
}
