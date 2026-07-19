import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

import { DndKitSpike } from "./dnd-kit-spike";
import { PangeaSpike } from "./pangea-spike";
import { PragmaticSpike } from "./pragmatic-spike";

/**
 * Queue-drag spike (#36 D6): both candidates over identical data and reorder
 * rules. The rubric verdict lands as a comment on #36; the loser (and its
 * packages) are deleted in the verdict commit.
 */
export default function SpikeDrag() {
  return (
    <div className="flex flex-col gap-4 p-6">
      <div className="flex flex-col gap-1">
        <h1 className="text-2xl">Queue-drag spike</h1>
        <p className="text-sm text-muted-foreground">
          ~500 virtualized rows, two levels. Try: reorder files within and across folders, drag a
          folder (its files follow), drag past the viewport edge to auto-scroll, and drive the
          keyboard path (dnd-kit and hello-pangea: focus a handle, Space, arrows, Space; pragmatic:
          the Move menu on each row).
        </p>
      </div>
      <Tabs defaultValue="hello-pangea">
        <TabsList>
          <TabsTrigger value="hello-pangea">hello-pangea</TabsTrigger>
          <TabsTrigger value="dnd-kit">dnd-kit</TabsTrigger>
          <TabsTrigger value="pragmatic">pragmatic-drag-and-drop</TabsTrigger>
        </TabsList>
        <TabsContent value="hello-pangea">
          <PangeaSpike />
        </TabsContent>
        <TabsContent value="dnd-kit">
          <DndKitSpike />
        </TabsContent>
        <TabsContent value="pragmatic">
          <PragmaticSpike />
        </TabsContent>
      </Tabs>
    </div>
  );
}
