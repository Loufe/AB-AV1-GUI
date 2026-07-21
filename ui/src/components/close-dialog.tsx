import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { forceStop, stopAfterCurrent } from "@/lib/ipc";
import { appStore, useAppStore } from "@/lib/store/app-store";

/**
 * Arms the quit and optionally issues a stop command. The quit intent stands
 * even when the command is rejected (the session may have ended between the
 * prompt and the click) — App closes the window as soon as the session is
 * idle either way.
 */
function chooseQuit(stop: (() => Promise<void>) | null): void {
  appStore.setState((state) => ({ ...state, closeRequested: false, quitAfterSession: true }));
  if (stop) {
    stop().catch((error: unknown) => {
      console.warn("stop command was not accepted", error);
    });
  } else {
    toast("CRFty will quit when the queue finishes.");
  }
}

/**
 * Prompt for a window close that arrived during an active session (#33 §12:
 * closing during active work prompts rather than hiding to tray). The shell
 * kept the window open; every choice except "keep converting" arms a quit
 * that App fires once the session reaches Idle.
 */
export function CloseDialog() {
  const open = useAppStore((state) => state.closeRequested);
  const dismiss = () =>
    appStore.setState((state) => ({ ...state, closeRequested: false, quitAfterSession: false }));
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          dismiss();
        }
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Conversion in progress</AlertDialogTitle>
          <AlertDialogDescription>
            CRFty is still working. Quit after the whole queue, after the current file, or right now
            — force-stopping discards the file in progress.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="sm:flex-col">
          <AlertDialogAction variant="outline" onClick={() => chooseQuit(null)}>
            Finish the queue, then quit
          </AlertDialogAction>
          <AlertDialogAction variant="outline" onClick={() => chooseQuit(stopAfterCurrent)}>
            Finish the current file, then quit
          </AlertDialogAction>
          <AlertDialogAction variant="destructive" onClick={() => chooseQuit(forceStop)}>
            Force stop and quit
          </AlertDialogAction>
          <AlertDialogCancel>Keep converting</AlertDialogCancel>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
