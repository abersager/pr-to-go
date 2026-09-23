-- The head's root .gitattributes, for deciding which files are generated
-- (linguist-generated). NULL when the repository has none.
ALTER TABLE pr_revision ADD COLUMN gitattributes TEXT;
