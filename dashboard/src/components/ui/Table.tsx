import type { HTMLAttributes, ThHTMLAttributes, TdHTMLAttributes } from 'react';
import { forwardRef } from 'react';
import { cn } from '@/utils/cn';

export interface TableProps extends HTMLAttributes<HTMLTableElement> {}

export const Table = forwardRef<HTMLTableElement, TableProps>(
  ({ className, ...props }, ref) => {
    return (
      <div className="relative w-full overflow-auto rounded-lg border border-black/20">
        <table
          ref={ref}
          className={cn('w-full caption-bottom text-sm', className)}
          {...props}
        />
      </div>
    );
  }
);

Table.displayName = 'Table';

export const TableHeader = forwardRef<HTMLTableSectionElement, HTMLAttributes<HTMLTableSectionElement>>(
  ({ className, ...props }, ref) => {
    return (
      <thead ref={ref} className={cn('bg-black/10 [&_tr]:border-b [&_tr]:border-black/20', className)} {...props} />
    );
  }
);

TableHeader.displayName = 'TableHeader';

export const TableBody = forwardRef<HTMLTableSectionElement, HTMLAttributes<HTMLTableSectionElement>>(
  ({ className, ...props }, ref) => {
    return (
      <tbody ref={ref} className={cn('[&_tr:last-child]:border-0', className)} {...props} />
    );
  }
);

TableBody.displayName = 'TableBody';

export const TableRow = forwardRef<HTMLTableRowElement, HTMLAttributes<HTMLTableRowElement>>(
  ({ className, ...props }, ref) => {
    return (
      <tr
        ref={ref}
        className={cn(
          'border-b border-black/20 transition-colors duration-150 hover:bg-black/10',
          className
        )}
        {...props}
      />
    );
  }
);

TableRow.displayName = 'TableRow';

export const TableHead = forwardRef<HTMLTableCellElement, ThHTMLAttributes<HTMLTableCellElement>>(
  ({ className, ...props }, ref) => {
    return (
      <th
        ref={ref}
        className={cn(
          'h-11 px-4 text-left align-middle text-xs font-medium uppercase tracking-wider text-gray-400 [&:has([role=checkbox])]:pr-0',
          className
        )}
        {...props}
      />
    );
  }
);

TableHead.displayName = 'TableHead';

export const TableCell = forwardRef<HTMLTableCellElement, TdHTMLAttributes<HTMLTableCellElement>>(
  ({ className, ...props }, ref) => {
    return (
      <td
        ref={ref}
        className={cn('p-4 align-middle text-gray-200 [&:has([role=checkbox])]:pr-0', className)}
        {...props}
      />
    );
  }
);

TableCell.displayName = 'TableCell';
