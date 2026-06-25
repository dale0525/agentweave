import * as Switch from "@radix-ui/react-switch";
import { useState } from "react";

import { skills as skillFixture } from "../data/fixtures";
import { SkillSummary } from "../types";

export function SettingsSkills(): JSX.Element {
  const [skillRows, setSkillRows] = useState<SkillSummary[]>(skillFixture);

  const updateSkill = (skillId: string, enabled: boolean) => {
    setSkillRows((currentSkills) =>
      currentSkills.map((skill) =>
        skill.id === skillId ? { ...skill, enabled } : skill
      )
    );
  };

  return (
    <section className="settings-panel" aria-labelledby="settings-skills-title">
      <div className="settings-panel-heading">
        <h2 id="settings-skills-title">Skills</h2>
        <p>Choose which assistant capabilities are available in chat.</p>
      </div>

      <div className="skill-list">
        {skillRows.map((skill) => (
          <article className="settings-skill-row" key={skill.id}>
            <div className="skill-copy">
              <div className="skill-title-row">
                <h3>{skill.name}</h3>
                <span className="skill-status">{skill.status}</span>
              </div>
              <p>{skill.description}</p>
            </div>
            <Switch.Root
              aria-label={skill.name}
              checked={skill.enabled}
              className="skill-switch"
              disabled={skill.status === "unavailable"}
              onCheckedChange={(enabled) => updateSkill(skill.id, enabled)}
            >
              <Switch.Thumb className="skill-switch-thumb" />
            </Switch.Root>
          </article>
        ))}
      </div>
    </section>
  );
}
