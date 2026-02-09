import type { HTMLAttributes } from 'react';
import { forwardRef } from 'react';
import { cn } from '@/utils/cn';

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  variant?: 'default' | 'success' | 'warning' | 'danger';
}

export const Badge = forwardRef<HTMLSpanElement, BadgeProps>(
  ({ className, variant = 'default', ...props }, ref) => {
    return (
      <span
        ref={ref}
        className={cn(
          'inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium',
          {
            'bg-white/[0.1] text-gray-300': variant === 'default',
            'bg-success/15 text-emerald-400': variant === 'success',
            'bg-amber-500/15 text-amber-400': variant === 'warning',
            'bg-error/15 text-red-400': variant === 'danger',
          },
          className
        )}
        {...props}
      />
    );
  }
);

Badge.displayName = 'Badge';
