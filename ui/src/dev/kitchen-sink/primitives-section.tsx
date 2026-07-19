import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

import { Section, ThemePair } from "./theme-pair";

export function PrimitivesSection() {
  return (
    <>
      <Section title="Action hierarchy (one primary per surface)">
        <ThemePair>
          <div className="flex items-center gap-2">
            <Button>Start</Button>
            <Button variant="outline">Add folder</Button>
            <Button variant="ghost">Clear completed</Button>
            <Button variant="destructive">Remove</Button>
            <Button disabled>Disabled</Button>
          </div>
        </ThemePair>
      </Section>

      <Section title="Form controls">
        <ThemePair>
          <div className="flex max-w-sm flex-col gap-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="ks-input">Filename suffix</Label>
              <Input id="ks-input" placeholder="-av1" />
            </div>
            <div className="flex items-center gap-2">
              <Checkbox id="ks-check" defaultChecked />
              <Label htmlFor="ks-check">Skip files below 720p</Label>
            </div>
            <div className="flex items-center gap-2">
              <Switch id="ks-switch" defaultChecked />
              <Label htmlFor="ks-switch">Anonymize paths</Label>
            </div>
          </div>
        </ThemePair>
      </Section>

      <Section title="Progress, tabs, skeleton, toast">
        <ThemePair>
          <div className="flex flex-col gap-4">
            <Progress value={62} />
            <Tabs defaultValue="a">
              <TabsList>
                <TabsTrigger value="a">Details</TabsTrigger>
                <TabsTrigger value="b">Streams</TabsTrigger>
              </TabsList>
              <TabsContent value="a" className="text-sm text-muted-foreground">
                Tab content A
              </TabsContent>
              <TabsContent value="b" className="text-sm text-muted-foreground">
                Tab content B
              </TabsContent>
            </Tabs>
            <div className="flex flex-col gap-2">
              <Skeleton className="h-4 w-3/5" />
              <Skeleton className="h-4 w-2/5" />
            </div>
            <div>
              <Button variant="outline" onClick={() => toast.success("Conversion finished")}>
                Fire toast
              </Button>
            </div>
          </div>
        </ThemePair>
      </Section>
    </>
  );
}
