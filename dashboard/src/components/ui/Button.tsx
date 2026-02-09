import type { ButtonHTMLAttributes } from 'react';
import { forwardRef } from 'react';
import { cn } from '@/utils/cn';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'danger' | 'ghost';
  size?: 'sm' | 'md' | 'lg';
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', ...props }, ref) => {
    return (
      <button
        ref={ref}
        className={cn(
          'inline-flex items-center justify-center rounded-lg font-medium transition-all duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-orange-500 focus-visible:ring-offset-2 focus-visible:ring-offset-bg-primary disabled:pointer-events-none disabled:opacity-50',
          {
            'bg-orange-500 text-white hover:bg-orange-600 active:bg-orange-700': variant === 'primary',
            'bg-white/[0.08] text-gray-200 hover:bg-white/[0.12] active:bg-white/[0.06] border border-white/[0.08]': variant === 'secondary',
            'bg-error/90 text-white hover:bg-error active:bg-error/80': variant === 'danger',
            'text-gray-400 hover:bg-white/[0.06] hover:text-gray-200 active:bg-transparent': variant === 'ghost',
          },
          {
            'h-8 px-3 text-xs': size === 'sm',
            'h-9 px-4 text-sm': size === 'md',
            'h-11 px-5 text-sm': size === 'lg',
          },
          className
        )}
        {...props}
      />
    );
  }
);

Button.displayName = 'Button';
