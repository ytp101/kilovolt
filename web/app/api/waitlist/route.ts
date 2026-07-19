import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';
import { createClient } from '@supabase/supabase-js';

const supabaseUrl = process.env.SUPABASE_URL || '';
const supabaseKey = process.env.SUPABASE_SERVICE_ROLE_KEY || process.env.SUPABASE_ANON_KEY || '';

const useSupabase = !!(supabaseUrl && supabaseKey);
const supabase = useSupabase ? createClient(supabaseUrl, supabaseKey) : null;

const isVercel = process.env.VERCEL === '1';
const waitlistFilePath = isVercel
  ? path.join('/tmp', 'waitlist.json')
  : path.join(process.cwd(), 'waitlist.json');

export async function POST(request: Request) {
  try {
    const { email } = await request.json();
    if (!email || !email.includes('@')) {
      return NextResponse.json({ error: 'Please enter a valid email address.' }, { status: 400 });
    }

    if (useSupabase && supabase) {
      const { error } = await supabase.from('waitlist').insert({ email });
      if (error) {
        if (error.code === '23505') { // PostgreSQL duplicate constraint violation
          return NextResponse.json({ message: 'You are already on the waitlist!' });
        }
        return NextResponse.json({ error: error.message }, { status: 500 });
      }
    } else {
      let list: string[] = [];
      try {
        const fileData = await fs.readFile(waitlistFilePath, 'utf8');
        list = JSON.parse(fileData);
      } catch (e) {
        // File doesn't exist yet
      }

      if (list.includes(email)) {
        return NextResponse.json({ message: 'You are already on the waitlist!' });
      }

      list.push(email);
      await fs.writeFile(waitlistFilePath, JSON.stringify(list, null, 2), 'utf8');
    }

    return NextResponse.json({ message: 'Successfully joined the waitlist!' });
  } catch (e) {
    return NextResponse.json({ error: 'Failed to process waitlist signup.' }, { status: 500 });
  }
}
