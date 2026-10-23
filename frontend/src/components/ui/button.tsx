import { Slot } from '@radix-ui/react-slot';
import { cva, type VariantProps } from 'class-variance-authority';
import { clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';
import type { ButtonHTMLAttributes } from 'react';

const variants = cva('ui-button inline-flex items-center justify-center font-medium disabled:pointer-events-none', {
  variants: { variant: { default: 'ui-button-primary', outline: 'ui-button-outline' } },
  defaultVariants: { variant: 'default' },
});
export function Button({ className, variant, asChild = false, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & VariantProps<typeof variants> & { asChild?: boolean }) {
  const Component = asChild ? Slot : 'button';
  return <Component className={twMerge(clsx(variants({ variant }), className))} {...props}/>;
}
