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
          'inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium',
          {
            'bg-bg-tertiary text-gray-300': variant === 'default',
            'bg-success/20 text-success': variant === 'success',
            'bg-yellow-500/20 text-yellow-400': variant === 'warning',
            'bg-error/20 text-error': variant === 'danger',
          },
          className
        )}
        {...props}
      />
    );
  }
);

Badge.displayName = 'Badge';
