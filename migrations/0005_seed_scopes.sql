insert into scope (name, description) values
    ('admin', 'Full administrative access'),
    ('read:accounts:own', 'Read your own account')
on conflict (name) do nothing;
