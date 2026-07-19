import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";

import { SettingContainer, SettingsGroup } from "./settings-primitives";

/**
 * Static structural skeleton (#36 D9 grouping): Conversion / Output /
 * Privacy / Dependencies. Controls are disabled until settings ride the
 * snapshot from the engine; no domain types are declared here.
 */
export function SettingsView() {
  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-4 p-6">
      <SettingsGroup title="Conversion">
        <SettingContainer label="Input folder" description="Folder scanned for videos to convert">
          <Button variant="outline" size="sm" disabled>
            Choose…
          </Button>
        </SettingContainer>
        <SettingContainer
          label="Hardware-accelerated decoding"
          description="Use the GPU to decode during quality sampling"
          last
        >
          <Switch disabled />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title="Output">
        <SettingContainer label="Output mode" description="Replace originals or write elsewhere">
          <Button variant="outline" size="sm" disabled>
            Replace
          </Button>
        </SettingContainer>
        <SettingContainer label="Filename suffix" description="Appended in suffix mode" last>
          <Input className="w-36" placeholder="-av1" disabled />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title="Privacy">
        <SettingContainer
          label="Anonymize paths in logs"
          description="Hash file and folder names in logs and history"
          last
        >
          <Switch disabled />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title="Dependencies">
        <SettingContainer label="FFmpeg" description="Version and updates appear here">
          <Button variant="outline" size="sm" disabled>
            Check
          </Button>
        </SettingContainer>
        <SettingContainer label="ab-av1" description="Version and updates appear here" last>
          <Button variant="outline" size="sm" disabled>
            Check
          </Button>
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
}
