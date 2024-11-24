import { StoryObj, Meta } from '@storybook/react';
import { useState } from "react";
import { Button } from "./Button";

const meta: Meta<typeof Button> = {
  title: "Example/Button",
  component: Button,
};

export default meta;
 
type Story = StoryObj<typeof Button>;

export const Primary: Story = {
  render: args => {
    let [count, setCount] = useState(0);
    return <><Button {...args} label="Increment" onClick={() => setCount(count + 1)} /> {count}</>;
  },
  args: {
    primary: true,
  },
};

export const Secondary: Story = {
  args: {
    label: "Button",
  },
};
