import React from 'react'
import { createRoot } from 'react-dom/client'
import '@/index.css'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Select } from '@/components/ui/select'
import { Card, CardHeader, CardContent } from '@/components/ui/card'
import { Dialog, DialogContent, DialogTitle, DialogFooter } from '@/components/ui/dialog'
function Fixture() {
  const [open, setOpen] = React.useState(false)
  const [value, setValue] = React.useState('')
  return (
    <main style={{ padding: 16 }}>
      <Card>
        <CardHeader>Control evidence</CardHeader>
        <CardContent>
          <label htmlFor="name">Name</label>
          <Input id="name" />
          <Select
            aria-label="Role"
            value={value}
            onChange={setValue}
            options={[{ value: 'long', label: 'Long role '.repeat(30) }]}
          />
          <Button onClick={() => setOpen(true)}>Open dialog</Button>
        </CardContent>
      </Card>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogTitle>Edit record</DialogTitle>
          <label htmlFor="detail">Detail</label>
          <Input id="detail" />
          <p>{'Multiline error message. '.repeat(50)}</p>
          <DialogFooter>
            <Button onClick={() => setOpen(false)}>Close</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </main>
  )
}
createRoot(document.getElementById('root')!).render(<Fixture />)
