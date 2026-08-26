import type { ComponentProps, ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface TooltipButtonProps extends ComponentProps<typeof Button> {
  tooltip: ReactNode;
  tooltipSide?: ComponentProps<typeof TooltipContent>["side"];
}

export function TooltipButton({
  children,
  disabled,
  tooltip,
  tooltipSide,
  ...buttonProps
}: TooltipButtonProps) {
  if (!tooltip) {
    return (
      <Button disabled={disabled} {...buttonProps}>
        {children}
      </Button>
    );
  }

  return (
    <Tooltip>
      {disabled ? (
        <TooltipTrigger render={<span className="inline-flex w-fit" />}>
          <Button disabled {...buttonProps}>
            {children}
          </Button>
        </TooltipTrigger>
      ) : (
        <TooltipTrigger render={<Button {...buttonProps} />}>
          {children}
        </TooltipTrigger>
      )}
      <TooltipContent side={tooltipSide}>{tooltip}</TooltipContent>
    </Tooltip>
  );
}
