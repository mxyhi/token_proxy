import { MoreHorizontal, Power, Pencil, Trash2, GripVertical } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { m } from "@/paraglide/messages.js";

import type { AgentConfig } from "./types";
import { AGENT_TOOL_META } from "./constants";

type AgentCardProps = {
  agent: AgentConfig;
  onSwitch: (id: string) => void;
  onEdit: (agent: AgentConfig) => void;
  onDelete: (id: string) => void;
  dragHandleProps?: React.HTMLAttributes<HTMLButtonElement>;
};

export function AgentCard({
  agent,
  onSwitch,
  onEdit,
  onDelete,
  dragHandleProps,
}: AgentCardProps) {
  const toolMeta = AGENT_TOOL_META[agent.type];

  return (
    <Card
      data-slot="agent-card"
      data-active={agent.isActive}
      className="group relative transition-all hover:shadow-md data-[active=true]:ring-2 data-[active=true]:ring-primary/50"
    >
      <CardContent className="flex items-center gap-3 p-4">
        {/* 拖拽手柄 */}
        {dragHandleProps && (
          <button
            type="button"
            data-slot="agent-card-drag-handle"
            className="cursor-grab touch-none text-muted-foreground/50 hover:text-muted-foreground active:cursor-grabbing"
            {...dragHandleProps}
          >
            <GripVertical className="size-4" />
          </button>
        )}

        {/* 主要内容 */}
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate font-medium">{agent.name}</span>
            {agent.isActive && (
              <Badge variant="default" className="shrink-0">
                {m.agents_active()}
              </Badge>
            )}
          </div>
          <p className="mt-0.5 text-xs text-muted-foreground">{toolMeta.label()}</p>
        </div>

        {/* 操作按钮 */}
        <div className="flex items-center gap-1">
          {/* 切换激活按钮 */}
          {!agent.isActive && (
            <Button
              variant="ghost"
              size="icon"
              className="size-8"
              onClick={() => onSwitch(agent.id)}
              title={m.agents_switch()}
            >
              <Power className="size-4" />
            </Button>
          )}

          {/* 更多操作 */}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" className="size-8">
                <MoreHorizontal className="size-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={() => onEdit(agent)}>
                <Pencil className="mr-2 size-4" />
                {m.common_edit()}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={() => onDelete(agent.id)}
              >
                <Trash2 className="mr-2 size-4" />
                {m.common_delete()}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </CardContent>
    </Card>
  );
}
