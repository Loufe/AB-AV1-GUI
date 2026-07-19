import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

import { DndKitReactSpike } from "./dnd-kit-react-spike";
import { DndKitSpike } from "./dnd-kit-spike";
import { PangeaSpike } from "./pangea-spike";
import { PragmaticSpike } from "./pragmatic-spike";

/**
 * Queue-drag spike (#36 D6): every candidate runs over identical data and
 * reorder rules. The rubric verdict lands as a comment on #36; the losers
 * (and their packages) are deleted in the verdict commit.
 *
 * Upstream health (checked 2026-07-19): pragmatic and @dnd-kit/react are
 * actively developed; dnd-kit legacy is frozen (2y) and hello-pangea is
 * dormant (~17m) — both kept only as feel references until the verdict.
 */
export default function SpikeDrag() {
  return (
    <div className="flex flex-col gap-4 p-6">
      <div className="flex flex-col gap-1">
        <h1 className="text-2xl">Queue-drag spike</h1>
        <p className="text-sm text-muted-foreground">
          ~500 rows, two levels. Try: reorder files within and across folders, drag a folder (its
          files follow), drag past the viewport edge to auto-scroll, and drive the keyboard path
          (pragmatic: the Move menu on each row; all others: focus a handle, Space, arrows, Space).
          The dnd-kit rewrite tab is non-virtualized (content-visibility hedge) — judge its scroll
          feel too.
        </p>
      </div>
      <Tabs defaultValue="dnd-kit-react">
        <TabsList>
          <TabsTrigger value="dnd-kit-react">dnd-kit rewrite</TabsTrigger>
          <TabsTrigger value="pragmatic">pragmatic</TabsTrigger>
          <TabsTrigger value="dnd-kit">dnd-kit legacy</TabsTrigger>
          <TabsTrigger value="hello-pangea">hello-pangea</TabsTrigger>
        </TabsList>
        <TabsContent value="dnd-kit-react">
          <DndKitReactSpike />
        </TabsContent>
        <TabsContent value="pragmatic">
          <PragmaticSpike />
        </TabsContent>
        <TabsContent value="dnd-kit">
          <DndKitSpike />
        </TabsContent>
        <TabsContent value="hello-pangea">
          <PangeaSpike />
        </TabsContent>
      </Tabs>
    </div>
  );
}
